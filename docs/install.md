# Installation

Two parts: `skym-server` on a machine you control, and `skym` on every monitored host. Both are static Linux binaries (x86_64 or aarch64, musl), built with:

```sh
cargo zigbuild --release -p skym-agent  --target x86_64-unknown-linux-musl
cargo zigbuild --release -p skym-server --target x86_64-unknown-linux-musl
```

## Server

`skym-server` serves plain HTTP and expects a reverse proxy in front of it for TLS.

1. Create a token for every host and every reader (person or AI agent):

   ```sh
   skym-server token
   ```

   It prints the token and its SHA-256. The configuration holds only the hash; hand the token itself to the host or the reader.
2. Write the configuration from [`deploy/server.example.toml`](../deploy/server.example.toml).
3. Run it, for example as a container built from [`deploy/Dockerfile`](../deploy/Dockerfile), with the configuration at `/etc/skym/server.toml` and a volume at `/data`:

   ```sh
   docker build -t skym-server -f deploy/Dockerfile <dir holding the binary>
   docker run -d --restart unless-stopped -p 127.0.0.1:7280:7280 \
     -v $PWD/server.toml:/etc/skym/server.toml:ro -v skym-data:/data skym-server
   ```
4. Point the reverse proxy at port 7280 and check `https://<server>/healthz`.

Changing the configuration (a new host, reader or mute) takes a restart.

## Agent

On each host, as root:

```sh
install -m 0755 skym /usr/local/bin/skym
useradd --system --no-create-home --shell /usr/sbin/nologin skym
usermod -aG docker skym                    # skip if the owner does not grant Docker access
install -d -m 0755 /etc/skym
install -m 0644 config.toml /etc/skym/config.toml           # from deploy/config.example.toml
install -m 0600 -o skym /dev/null /etc/skym/token
echo '<host token>' > /etc/skym/token
install -d -o skym -m 0750 /var/lib/skym
sudo -u skym skym doctor                   # every check should pass
install -m 0644 skym.service /etc/systemd/system/skym.service   # deploy/skym.service
systemctl daemon-reload && systemctl enable --now skym
journalctl -u skym -f
```

The unit runs `skym agent` as the `skym` user with read-only access to the system, at low CPU and I/O priority, capped at 20% of one CPU and 256 MB. See [`deploy/skym.service`](../deploy/skym.service).

Membership in the `docker` group is effectively root access; see the [security model](architecture.md#security-model).

## AI agents

Give the agent the [skill](../skill/SKILL.md) and a reader token, as `SKYM_URL` and `SKYM_TOKEN` in its environment or in `~/.config/skym/env`.
