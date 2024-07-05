FROM rust:alpine AS builder

RUN apk update && apk add musl-dev fuse3-dev fuse3-static pkgconfig

WORKDIR /usr/src/ouisync
COPY . .

RUN cargo build --package ouisync-cli --release --bins && \
    cp target/release/ouisync /usr/bin/

FROM scratch AS runtime

EXPOSE 20209/tcp 20209/udp

ENV PATH=/                       \
    OUISYNC_CONFIG_DIR=/config   \
    OUISYNC_STORE_DIR=/store     \
    OUISYNC_MOUNT_DIR=/mount     \
    OUISYNC_SOCKET=/ouisync.sock \
    RUST_LOG=ouisync=debug

COPY --from=builder /usr/bin/ouisync /

RUN ["ouisync", "bind", "quic/0.0.0.0:20209", "quic/[::]:20209", "tcp/[::]:20209"]

ENTRYPOINT ["ouisync"]
CMD ["start"]
