FROM docker.io/rust:1.90-slim-bookworm AS builder

WORKDIR /usr/src/app
COPY . .

RUN apt update && apt install -y make gcc

RUN cargo build --release --package vimputti-manager --bins
RUN cargo build --release --example simple_controller
RUN cargo build --release --package vimputti-shim
RUN cargo build --release --example create_test_device
RUN make test_shim


FROM docker.io/debian:bookworm-slim

WORKDIR /app
COPY --from=builder /usr/src/app/target/release/vimputti-manager .
COPY --from=builder /usr/src/app/target/release/examples/simple_controller ./simple_controller
COPY --from=builder /usr/src/app/target/release/libvimputti_shim.so ./libvimputti_shim.so
COPY --from=builder /usr/src/app/target/release/examples/create_test_device ./create_test_device
COPY --from=builder /usr/src/app/test_shim ./test_shim

ENTRYPOINT ["vimputti-manager"]
