#pragma once
#include <stddef.h>
#include <stdint.h>

/* The console link to a host: USB Serial/JTAG on the chips that have it, the
   console UART behind a USB bridge on the original ESP32. */
void bridge_transport_init(void);
int bridge_transport_read(void *buffer, size_t length);
void bridge_transport_write(const void *buffer, size_t length);
uint32_t bridge_transport_dropped(void);
const char *bridge_transport_name(void);
/* Takes effect once everything already queued has left at the old rate. A no-op
   where the link has no line rate. */
void bridge_transport_set_baud(int baud);
/* Block until queued output has left, within a bound. */
void bridge_transport_flush(void);
