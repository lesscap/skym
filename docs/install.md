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
     -v $PWD/server.toml:/etc/skym/server.toml:ro -v skym-data:/data \
     -v /etc/ssl/certs:/etc/ssl/certs:ro skym-server
   ```

   The image holds nothing but the binary: the host's CA certificates, mounted read-only, let it verify the https endpoints it probes.
4. Point the reverse proxy at port 7280 and check `https://<server>/healthz`.

Changing the configuration (a new host, reader, mute, application or endpoint) takes a restart.

### Applications

skym lists every application it finds: a compose project, or a container outside compose or a systemd unit on its own. `[[apps]]` describes the ones you care about:

```toml
[[apps]]
id = "web-1/shop"                 # <host>/<project>, or <host>/-/<container>, <host>/_systemd/<unit>
name = "Shop"
env = "prod"                      # any text; prod, pre and test sort first
note = "The web shop; its test copy is web-1/shop-test"

[[apps.probes]]                   # the application's URLs, probed like [[endpoints]]
url = "https://shop.example.com/healthz"
headers = { Authorization = "Bearer <probe token>" }
```

A listed container application that disappears from its host opens `APP_MISSING`: list what should be running, leave out what may come and go. systemd units can be listed too, for their name, note and probes; an absent unit is already `WORKLOAD_DOWN`. `APP_MISSING` needs agents that report complete container listings (this version or later). A container outside compose is only tracked with a restart policy (`always`, `unless-stopped` or `on-failure`); without one it is short-lived to skym, so do not list it.

### Endpoints

`[[endpoints]]` in the configuration lists URLs that belong to no application, which the server probes from its own host, once per report interval: whether they answer, and when their certificates expire. See the examples in [`deploy/server.example.toml`](../deploy/server.example.toml) and the [judgement rules](judgement.md).

For an application you run, a URL made for probing tells more than its home page:

- `GET`, answering 2xx when the application can serve its users (it can reach what it needs), 503 when it cannot;
- fast (well under 10 seconds) and free of side effects, since it is called every minute;
- open, or behind a token of its own that grants nothing else. Put the token in the probe's `headers`; the file then holds a secret, so keep it mode 0600 and out of version control.

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

## Viewing from your own machine

`skym-view` is a terminal view of the server, read-only, for people:

```sh
cargo install --path crates/view
skym-view                      # SKYM_URL and SKYM_TOKEN from the environment or ~/.config/skym/env
skym-view --server https://skym.example.com
```

`~/.config/skym/env` holds `KEY=VALUE` lines (`SKYM_URL=…`, `SKYM_TOKEN=<reader token>`); keep it mode 0600. Press `?` in the view for its keys.

## Upgrading

Install the new `skym-view` first, then the server, then the agents. Newer servers may send subjects and fields older views do not know; newer agents may send fields older servers ignore.

## AI agents

Give the agent the [skill](../skill/SKILL.md) and a reader token, as `SKYM_URL` and `SKYM_TOKEN` in its environment or in `~/.config/skym/env`.
