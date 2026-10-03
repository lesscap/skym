# skym-server from a static (musl) binary built beforehand:
#   cargo zigbuild --release -p skym-server --target x86_64-unknown-linux-musl
FROM scratch
COPY skym-server /skym-server
VOLUME /data
EXPOSE 7280
ENTRYPOINT ["/skym-server", "serve", "--config", "/etc/skym/server.toml"]
