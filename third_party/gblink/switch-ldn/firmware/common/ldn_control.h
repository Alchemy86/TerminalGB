#pragma once
#include <stdbool.h>
#include "esp_netif.h"
void ldn_control_init(esp_netif_t *netif, const unsigned char host[6]);
void ldn_control_link(bool connected);
void ldn_control_poll(void);
void ldn_control_sniff(const unsigned char *frame, size_t length);
void ldn_control_target(const unsigned char host[6]);

/* Action-frame path for on-device room authentication. */
int ldn_control_tx_action(const uint8_t *frame, size_t length);
void ldn_control_set_action_handler(void (*handler)(const uint8_t source[6], const uint8_t *body, size_t length));
bool ldn_control_connected(void);
void ldn_control_mac(uint8_t out[6]);
/* Firmware version reported by LDN_INFO. */
#define BRIDGE_VERSION "2.0.2"
/* Switch the console line rate if LDN_BAUD asked for it; call once the reply has gone. */
void ldn_control_apply_baud(void);
/* Push everything queued for the adapter and the console out before a restart. */
void ldn_control_flush_adapter(void);
