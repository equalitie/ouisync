FROM rust:slim-trixie AS builder

RUN apt-get update && apt-get upgrade -y && \
    apt-get install -y \
        cmake \
        # findutils \
        git \
        jq \
        libclang-dev \
        # libc-dbg \
        libglib2.0-dev \
        # make \
        # netbase \
        python3 \
        # python3-networkx \
        xz-utils \
        # util-linux \
        # gcc \
        g++ \
        && \
    apt-get clean

# Build and install shadow
RUN git clone https://github.com/shadow/shadow.git /usr/src/shadow
WORKDIR /usr/src/shadow
RUN ./setup build --clean --test --prefix /opt/shadow && ./setup install

# Build ouisync tests and utilities
WORKDIR /usr/src/ouisync
COPY --from=source . .

RUN cargo build --package ouisync --release --test sync --message-format json \
    | jq --raw-output '.executable | select( . != null)' \
    | xargs -I{} mv {} target/release/ouisync-test-sync
RUN cargo build --package ouisync-stress-test --release
RUN cargo build --package shadow-runner       --release


FROM debian:trixie-slim AS runner

RUN apt-get update && apt-get upgrade -y && \
    apt-get install -y \
        libglib2.0-0 \
        && \
    apt-get clean

COPY --from=builder /opt/shadow/bin/*                                   /usr/bin/
COPY --from=builder /opt/shadow/lib/*                                   /usr/lib/
COPY --from=builder /usr/src/ouisync/target/release/ouisync-test-sync   /usr/bin/
COPY --from=builder /usr/src/ouisync/target/release/stress-test         /usr/bin/
COPY --from=builder /usr/src/ouisync/target/release/shadow-runner       /usr/bin/

CMD stress-test \
    --exe ouisync-test-sync \
    --runner shadow-runner \
    --concurrency ${CONCURRENCY:-1} \
    --slow 10m
