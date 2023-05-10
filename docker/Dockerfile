FROM rust:alpine as builder

RUN apk update && apk add fuse3-dev g++ pkgconfig

WORKDIR /usr/src/ouisync
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/src/ouisync/target   \
    cargo build --package ouisync-cli --release --bins; \
    cp target/release/ouisync /usr/bin/

FROM scratch as runtime

EXPOSE 443/tcp
EXPOSE 2000/tcp 2000/udp

ENV PATH=/                       \
    OUISYNC_CONFIG_DIR=/config   \
    OUISYNC_STORE_DIR=/store     \
    OUISYNC_MOUNT_DIR=/mount     \
    OUISYNC_SOCKET=/ouisync.sock \
    RUST_LOG=ouisync=debug

COPY --from=builder /usr/bin/ouisync /

RUN ["ouisync", "bind", "quic/0.0.0.0:2000", "quic/[::]:2000", "tcp/[::]:2000"]
RUN ["ouisync", "bind-rpc", "--delay", "[::]:443"]

ENTRYPOINT ["ouisync"]
CMD ["start"]
