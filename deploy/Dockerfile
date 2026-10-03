# skym-server from a static (musl) binary built beforehand:
#   cargo zigbuild --release -p skym-server --target x86_64-unknown-linux-musl
# No CA certificates inside: mount the host's (-v /etc/ssl/certs:/etc/ssl/certs:ro) so https
# endpoint probes can verify certificates.
FROM scratch
COPY skym-server /skym-server
VOLUME /data
EXPOSE 7280
ENTRYPOINT ["/skym-server", "serve", "--config", "/etc/skym/server.toml"]
