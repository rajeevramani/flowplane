# Secret KEK Rotation

> Audience: operators, platform-engineers · Status: stable

Flowplane encrypts stored `SecretSpec` values with AES-256-GCM. Each secret row stores the `encryption_key_id` used for that ciphertext.

## Environment Contract

- `FLOWPLANE_SECRET_ENCRYPTION_KEY`: active 32-byte key material. Accepts raw 32-byte text, standard base64, or URL-safe base64 without padding.
- `FLOWPLANE_SECRET_ENCRYPTION_KEY_ID`: id written to new/rotated secrets. Defaults to `default`.
- `FLOWPLANE_SECRET_ENCRYPTION_KEYS`: optional JSON object of retired keys, keyed by `encryption_key_id`, used for decrypting old ciphertext. Example:

```json
{
  "2026-06-primary": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "2026-03-primary": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
}
```

## Rotation Procedure

1. Pick a new key id, for example `2026-07-primary`.
2. Move the current key into `FLOWPLANE_SECRET_ENCRYPTION_KEYS` under its current `FLOWPLANE_SECRET_ENCRYPTION_KEY_ID`.
3. Set `FLOWPLANE_SECRET_ENCRYPTION_KEY` to the new key material.
4. Set `FLOWPLANE_SECRET_ENCRYPTION_KEY_ID` to the new key id.
5. Restart or roll the control plane so all writers and xDS rebuilders use the same keyring.
6. Rotate existing Flowplane secrets through the normal secret rotate path. New ciphertext will be written with the new key id.
7. After all rows using the retired key id have been rotated and xDS has rebuilt successfully, remove that retired key from `FLOWPLANE_SECRET_ENCRYPTION_KEYS`.

Never remove a retired key while any `secrets.encryption_key_id` still references it; those secrets will be skipped during SDS rebuild until the key is restored or the secret is rotated.

## Runnable drill

This drill rotates one secret from key id `2026-06-primary` to
`2026-07-primary`. Run it first in a non-production environment with a real
PostgreSQL database and the same deployment mechanism you use to roll the
control plane.

Start with the old key active:

```bash
export FLOWPLANE_SECRET_ENCRYPTION_KEY_ID="2026-06-primary"
export FLOWPLANE_SECRET_ENCRYPTION_KEY="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
unset FLOWPLANE_SECRET_ENCRYPTION_KEYS
```

Create or choose a write-only secret and record its current revision and key id:

```bash
flowplane secret get openai-key --team payments -o json
```

The response is metadata only. It should show the current `revision` and
`encryption_key_id`; it never returns the secret value.

Roll the control plane with the new active key and the retired keyring:

```bash
export FLOWPLANE_SECRET_ENCRYPTION_KEY_ID="2026-07-primary"
export FLOWPLANE_SECRET_ENCRYPTION_KEY="cccccccccccccccccccccccccccccccc"
export FLOWPLANE_SECRET_ENCRYPTION_KEYS='{
  "2026-06-primary": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}'
```

After the rollout, prove that the old ciphertext is still decryptable by
rotating the secret value through the normal write path. The rotate request uses
the current revision from `secret get`:

```bash
cat >secret-rotate.json <<'JSON'
{
  "spec": {
    "type": "generic_secret",
    "secret": "<base64 of the new raw secret value>"
  }
}
JSON

flowplane secret rotate openai-key \
  --team payments \
  --revision <current-revision> \
  --file secret-rotate.json
```

Verify the metadata now shows the new key id and an incremented revision:

```bash
flowplane secret get openai-key --team payments -o json
```

Repeat `secret get` / `secret rotate` for every secret whose metadata still
shows the retired key id. You can list metadata with:

```bash
flowplane secret list --team payments -o json
```

Only after every secret has moved off `2026-06-primary` should you remove that
entry from `FLOWPLANE_SECRET_ENCRYPTION_KEYS` and roll the control plane again.

## Verification checklist

- `flowplane secret get` before rotation shows the old `encryption_key_id`.
- The control plane starts with the new active key plus the retired keyring.
- `flowplane secret rotate` succeeds while the old key is present in
  `FLOWPLANE_SECRET_ENCRYPTION_KEYS`.
- `flowplane secret get` after rotation shows the new
  `encryption_key_id`.
- `flowplane secret list` shows no rows still using the retired key id before
  you remove it from the keyring.
