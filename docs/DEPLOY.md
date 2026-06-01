# Deploying Nimbus on a private VPS

Nimbus is a single binary with the web UI embedded, so deployment is light —
no PHP, no database server, no Redis. The recommended production setup is the
binary behind [Caddy](https://caddyserver.com) for automatic HTTPS.

## Option 1 — Docker Compose (recommended)

Prerequisites: a VPS with Docker, and a domain's DNS `A`/`AAAA` record pointed
at the server's IP.

```bash
git clone https://github.com/An3treiu/Nimbus.git && cd Nimbus
cp .env.example .env
# edit .env: set NIMBUS_DOMAIN, NIMBUS_DRIVE_OWNER, NIMBUS_DRIVE_REPO,
# a strong NIMBUS_ADMIN_TOKEN (openssl rand -hex 32), and a GitHub token/clientid
docker compose up -d
```

Caddy obtains a Let's Encrypt certificate for `NIMBUS_DOMAIN` and proxies to
Nimbus. Open `https://<your-domain>` and enter your admin token when prompted.

Data (the SQLite cache + encrypted OAuth token + one-time recovery key) lives in
the `nimbus-data` volume at `/data`. **Back up `/data`** — though GitHub remains
the source of truth, so the cache can always be rebuilt with `POST /api/sync`.

## Option 2 — Bare-metal binary + your own proxy

```bash
# Download a release binary (or `cargo build --release` after `cd web && npm run build`).
export NIMBUS_BIND_ADDR=127.0.0.1:8080
export NIMBUS_ADMIN_TOKEN=$(openssl rand -hex 32)
export NIMBUS_DRIVE_OWNER=... NIMBUS_DRIVE_REPO=... NIMBUS_GITHUB_TOKEN=...
./nimbus
```

Put it behind nginx/Caddy/Traefik terminating TLS and proxying to `127.0.0.1:8080`.
Run it under systemd for restarts:

```ini
# /etc/systemd/system/nimbus.service
[Unit]
Description=Nimbus
After=network.target

[Service]
EnvironmentFile=/etc/nimbus.env
ExecStart=/usr/local/bin/nimbus
Restart=always
DynamicUser=yes
StateDirectory=nimbus

[Install]
WantedBy=multi-user.target
```

## Security checklist

- **Always** set `NIMBUS_ADMIN_TOKEN` for any non-loopback deployment — Nimbus
  refuses to bind to a public address without it.
- Terminate TLS at the proxy; never expose `:8080` directly.
- Enable client-side encryption (`NIMBUS_ENCRYPTION_PASSPHRASE`) so GitHub only
  ever stores ciphertext; store the printed recovery key somewhere safe.
- Health check: `GET /healthz` returns `200 ok` (unauthenticated).

## Updating

```bash
git pull && docker compose up -d --build   # compose
# or replace the binary and restart the service
```
