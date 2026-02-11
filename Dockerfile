FROM rust:latest AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release --bin fila-server --bin fila

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/fila-server /usr/local/bin/
COPY --from=builder /build/target/release/fila /usr/local/bin/
EXPOSE 5555
ENTRYPOINT ["fila-server"]
