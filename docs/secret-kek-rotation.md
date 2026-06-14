# Secret KEK Rotation

Flowplane encrypts stored `SecretSpec` values with AES-256-GCM. Each secret row stores the
`encryption_key_id` used for that ciphertext.

## Environment Contract

- `FLOWPLANE_SECRET_ENCRYPTION_KEY`: active 32-byte key material. Accepts raw 32-byte text,
  standard base64, or URL-safe base64 without padding.
- `FLOWPLANE_SECRET_ENCRYPTION_KEY_ID`: id written to new/rotated secrets. Defaults to
  `default`.
- `FLOWPLANE_SECRET_ENCRYPTION_KEYS`: optional JSON object of retired keys, keyed by
  `encryption_key_id`, used for decrypting old ciphertext. Example:

```json
{
  "2026-06-primary": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "2026-03-primary": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
}
```

## Rotation Procedure

1. Pick a new key id, for example `2026-07-primary`.
2. Move the current key into `FLOWPLANE_SECRET_ENCRYPTION_KEYS` under its current
   `FLOWPLANE_SECRET_ENCRYPTION_KEY_ID`.
3. Set `FLOWPLANE_SECRET_ENCRYPTION_KEY` to the new key material.
4. Set `FLOWPLANE_SECRET_ENCRYPTION_KEY_ID` to the new key id.
5. Restart or roll the control plane so all writers and xDS rebuilders use the same keyring.
6. Rotate existing Flowplane secrets through the normal secret rotate path. New ciphertext will
   be written with the new key id.
7. After all rows using the retired key id have been rotated and xDS has rebuilt successfully,
   remove that retired key from `FLOWPLANE_SECRET_ENCRYPTION_KEYS`.

Never remove a retired key while any `secrets.encryption_key_id` still references it; those
secrets will be skipped during SDS rebuild until the key is restored or the secret is rotated.
