/*
 * sensors_wrapper.c - flat C API over libsensors for rs-sensors.
 *
 * Exposes a stable array of sensor_entry structs (chip, label, type,
 * input value, min/max/crit/lcrit thresholds, alarm flag) that Rust
 * can read safely each refresh cycle. All strings are owned copies.
 */
#include <sensors/sensors.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <math.h>

typedef struct {
    char *chip;      /* owned copy of chip name */
    char *label;     /* owned copy of feature label */
    int type;        /* sensors_feature_type */
    double input;    /* current value; NAN if unavailable */
    int has_min;     double min;
    int has_max;     double max;
    int has_lcrit;   double lcrit;
    int has_crit;    double crit;
    int alarm;       /* 1 if alarm subfeature is non-zero */
} sensor_entry;

static sensor_entry *g_entries = NULL;
static int g_count = 0;
static int g_ready = 0;

static char *dupstr(const char *s) {
    if (!s) return NULL;
    size_t n = strlen(s) + 1;
    char *p = (char *)malloc(n);
    if (p) memcpy(p, s, n);
    return p;
}

/* Read a subfeature of given type; returns 1 on success, 0 otherwise. */
static int sub_value(const sensors_chip_name *chip,
                     const sensors_feature *feature,
                     sensors_subfeature_type st,
                     double *out) {
    const sensors_subfeature *sf = sensors_get_subfeature(chip, feature, st);
    if (!sf) return 0;
    return sensors_get_value(chip, sf->number, out) == 0;
}

int rs_sensors_init(char **err) {
    int rc = sensors_init(NULL);
    if (rc != 0) {
        if (err) *err = "sensors_init failed (is libsensors installed / config present?)";
    } else {
        g_ready = 1;
    }
    return rc;
}

void rs_sensors_cleanup(void) {
    if (g_entries) {
        for (int i = 0; i < g_count; i++) {
            free(g_entries[i].chip);
            free(g_entries[i].label);
        }
        free(g_entries);
        g_entries = NULL;
    }
    g_count = 0;
    if (g_ready) {
        sensors_cleanup();
        g_ready = 0;
    }
}

/* Rebuild the entries array from scratch; sets *count to the new total. */
void rs_sensors_refresh(int *count) {
    /* free previous generation */
    if (g_entries) {
        for (int i = 0; i < g_count; i++) {
            free(g_entries[i].chip);
            free(g_entries[i].label);
        }
        free(g_entries);
        g_entries = NULL;
    }
    g_count = 0;

    /* first pass: count how many feature entries we'll produce */
    int tmp = 0;
    int cap = 0;
    const sensors_chip_name *chip = NULL;
    while ((chip = sensors_get_detected_chips(NULL, &tmp)) != NULL) {
        int fnr = 0;
        const sensors_feature *feature = NULL;
        while ((feature = sensors_get_features(chip, &fnr)) != NULL) cap++;
    }

    if (cap == 0) {
        *count = 0;
        return;
    }

    g_entries = (sensor_entry *)malloc(sizeof(sensor_entry) * cap);
    if (!g_entries) {
        *count = 0;
        return;
    }

    int idx = 0;
    tmp = 0;
    while ((chip = sensors_get_detected_chips(NULL, &tmp)) != NULL) {
        char chipname[512] = {0};
        sensors_snprintf_chip_name(chipname, sizeof(chipname), chip);

        int fnr = 0;
        const sensors_feature *feature = NULL;
        while ((feature = sensors_get_features(chip, &fnr)) != NULL) {
            sensor_entry *e = &g_entries[idx];
            e->chip = dupstr(chipname);
            e->label = sensors_get_label(chip, feature); /* malloc'd, keep */
            e->type = feature->type;
            e->input = NAN;
            e->has_min = e->has_max = e->has_lcrit = e->has_crit = 0;
            e->min = e->max = e->lcrit = e->crit = NAN;
            e->alarm = 0;

            /* main input value: offset 0 for every feature type */
            sensors_subfeature_type input_st =
                (sensors_subfeature_type)(feature->type << 8);
            double v;
            if (sub_value(chip, feature, input_st, &v)) e->input = v;

            switch (feature->type) {
                case SENSORS_FEATURE_IN:
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_IN_MIN, &v))   { e->has_min = 1;  e->min = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_IN_MAX, &v))   { e->has_max = 1;  e->max = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_IN_LCRIT, &v)) { e->has_lcrit = 1; e->lcrit = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_IN_CRIT, &v))  { e->has_crit = 1;  e->crit = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_IN_ALARM, &v))  e->alarm = (v != 0.0);
                    break;
                case SENSORS_FEATURE_FAN:
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_FAN_MIN, &v))  { e->has_min = 1; e->min = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_FAN_MAX, &v))  { e->has_max = 1; e->max = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_FAN_ALARM, &v)) e->alarm = (v != 0.0);
                    break;
                case SENSORS_FEATURE_TEMP:
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_TEMP_MIN, &v))  { e->has_min = 1; e->min = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_TEMP_MAX, &v))  { e->has_max = 1; e->max = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_TEMP_LCRIT, &v)) { e->has_lcrit = 1; e->lcrit = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_TEMP_CRIT, &v))  { e->has_crit = 1; e->crit = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_TEMP_ALARM, &v)) e->alarm = (v != 0.0);
                    break;
                case SENSORS_FEATURE_POWER:
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_POWER_MIN, &v))  { e->has_min = 1; e->min = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_POWER_MAX, &v))  { e->has_max = 1; e->max = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_POWER_LCRIT, &v)) { e->has_lcrit = 1; e->lcrit = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_POWER_CRIT, &v))  { e->has_crit = 1; e->crit = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_POWER_ALARM, &v)) e->alarm = (v != 0.0);
                    break;
                case SENSORS_FEATURE_ENERGY:
                    break;
                case SENSORS_FEATURE_CURR:
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_CURR_MIN, &v))  { e->has_min = 1; e->min = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_CURR_MAX, &v))  { e->has_max = 1; e->max = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_CURR_LCRIT, &v)) { e->has_lcrit = 1; e->lcrit = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_CURR_CRIT, &v))  { e->has_crit = 1; e->crit = v; }
                    if (sub_value(chip, feature, SENSORS_SUBFEATURE_CURR_ALARM, &v)) e->alarm = (v != 0.0);
                    break;
                default:
                    break;
            }
            idx++;
        }
    }

    g_count = idx;
    *count = g_count;
}

const sensor_entry *rs_sensors_entry(int idx) {
    if (idx < 0 || idx >= g_count) return NULL;
    return &g_entries[idx];
}

int rs_sensors_num_entries(void) {
    return g_count;
}

const char *rs_sensors_version(void) {
    return libsensors_version;
}
