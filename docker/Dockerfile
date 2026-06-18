FROM rust:1.80-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY . .
RUN cargo build --release --bin rust-can

FROM alpine:3.20
RUN apk add --no-cache git ca-certificates
COPY --from=builder /build/target/release/rust-can /usr/local/bin/rust-can
LABEL org.opencontainers.image.title="rust-can"
LABEL org.opencontainers.image.source="https://github.com/SoundMatt/rust-CAN"
LABEL org.opencontainers.image.licenses="MPL-2.0"
LABEL io.relay.tool="rust-can"
LABEL io.relay.language="rust"
LABEL io.relay.binary="rust-can"
LABEL io.relay.spec-version="1.1"
ENTRYPOINT ["/usr/local/bin/rust-can"]
