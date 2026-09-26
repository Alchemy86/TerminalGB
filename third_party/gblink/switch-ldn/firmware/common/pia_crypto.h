#pragma once
#include "pia_bin.h"

/* Pia transport crypto and framing: the AES-GCM datagram envelope, zstd body
   compression, and the nested message encoding. Mirrors the host's PiaCrypto. */

#define PIA_MAX_DATAGRAM 1472
#define PIA_MAX_BODY 2048
/* A decompressed datagram body. The Switch re-sends every reliable frame its partner
   has not acknowledged in one datagram: such batches were observed to top out at
   74 messages and 7.2 KB, from 381 bytes on the wire. */
#define PIA_MAX_INFLATE 12288
#define PIA_MAX_MESSAGES 16

typedef struct
{
    uint8_t key[16];
    uint32_t net_id;
} pia_crypto_t;

typedef struct
{
    uint8_t protocol;
    uint8_t flags;
    bool has_flags;
    const uint8_t *payload;
    uint16_t length;
} pia_message_t;

/* AES-ECB single block. `encrypt` false unwraps, matching the host's default. */
void pia_ecb(const uint8_t key[16], const uint8_t in[16], uint8_t out[16], bool encrypt);
/* AES-GCM with a caller-chosen tag length. Returns false when a tag does not verify. */
bool pia_gcm(const uint8_t *key, size_t key_len, const uint8_t *nonce, size_t nonce_len,
             const uint8_t *aad, size_t aad_len, const uint8_t *in, size_t in_len,
             uint8_t *out, uint8_t *tag, size_t tag_len, bool encrypt);
/* AES-CTR with a 4-byte prefix counter, as the v1 advertisement uses. */
void pia_ctr(const uint8_t key[16], const uint8_t nonce4[4], const uint8_t *in, size_t len, uint8_t *out);
void pia_sha256(const uint8_t *data, size_t len, uint8_t out[32]);
void pia_hmac_sha256(const uint8_t *key, size_t key_len, const uint8_t *data, size_t len, uint8_t out[32]);

void pia_crypto_init(pia_crypto_t *c, const uint8_t ssid[16]);

/* Decrypt a received datagram into `out`. Returns the plaintext length, or -1. */
int pia_decrypt(const pia_crypto_t *c, const uint8_t *datagram, size_t length,
                const char *source_ip, uint8_t *out, size_t out_cap);
/* Build a datagram from an already-assembled body. Returns its length, or -1. */
int pia_encrypt(const pia_crypto_t *c, const uint8_t *body, size_t body_len, const char *source_ip,
                uint16_t dst, uint16_t src, uint16_t packet, uint64_t nonce, uint8_t flags,
                uint8_t footer, uint8_t *out, size_t out_cap);

/* zstd: the Switch compresses most datagrams, so decoding is required. Encoding is not:
   a frame of raw blocks is valid zstd and carries the canonical header Pia expects. */
/* Claim the zstd decompression context while the heap is still whole: it needs ~94 KB
   contiguous and, once Wi-Fi is initialised, the largest free block is about half that. */
bool pia_crypto_prepare(void);
size_t pia_crypto_dctx_size(void);

/* Returns the decompressed length; -1 when the context is missing or a plain body does
   not fit; -(1000 + ZSTD_ErrorCode) when zstd rejects the frame (1070 = output too small). */
int pia_decompress(const uint8_t *in, size_t len, uint8_t *out, size_t out_cap);
int pia_compress_raw(const uint8_t *in, size_t len, uint8_t *out, size_t out_cap);

/* Message list encoding: [bits][flags?][size:2][protocol][payload]. */
int pia_message_encode(const pia_message_t *m, uint8_t *out, size_t out_cap);
int pia_messages_decode(const uint8_t *data, size_t len, pia_message_t *out, int max);
/* The same decoding one message at a time, with no limit on how many a body holds. */
typedef struct { const uint8_t *data; size_t len, pos; int size, proto; uint8_t flags; } pia_message_iter_t;
void pia_message_iter_init(pia_message_iter_t *it, const uint8_t *data, size_t len);
bool pia_message_next(pia_message_iter_t *it, pia_message_t *out);
