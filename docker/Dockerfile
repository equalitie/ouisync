FROM rust:alpine AS builder

RUN apk update && apk add musl-dev fuse3-dev fuse3-static pkgconfig

WORKDIR /usr/src/ouisync
COPY . .

RUN cargo build --package ouisync-cli --release --bins && \
    cp target/release/ouisync /usr/bin/

FROM scratch AS runtime

EXPOSE 20209/tcp 20209/udp

ENV PATH=/                     \
    OUISYNC_CONFIG_DIR=/config \
    RUST_LOG=ouisync=debug

COPY --from=builder /usr/bin/ouisync /
COPY docker/config /config

ENTRYPOINT ["ouisync"]
CMD ["start"]
