#pragma once
#include <stdbool.h>
#include "esp_err.h"

const char *ldn_session_ssid(void);
esp_err_t ldn_session_configure(const char *ssid, const char *bssid, const char *key, unsigned channel);
esp_err_t ldn_session_scan(unsigned channel);
void ldn_session_stop(void);
void ldn_control_target(const unsigned char host[6]);

/* Strength of the last LDN action frame heard, and min/avg/max since the last call. */
int8_t ldn_probe_last_rssi(void);
void ldn_probe_rssi_window(int *count, int *average, int *low, int *high);
/* Select the external (true) or onboard antenna; false if the board has no switch. */
bool bridge_board_antenna(bool external);
