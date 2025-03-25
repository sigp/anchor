ARG RUST_VERSION=1.85.1
FROM rust:${RUST_VERSION}-bullseye AS builder
RUN apt update && apt dist-upgrade -y && apt install -y cmake libclang-dev
COPY . anchor
ARG FEATURES
ARG PROFILE=release
ARG CARGO_USE_GIT_CLI=true
ENV FEATURES=$FEATURES
ENV PROFILE=$PROFILE
ENV CARGO_NET_GIT_FETCH_WITH_CLI=$CARGO_USE_GIT_CLI
RUN cd anchor && make

FROM ubuntu:24.04
ENTRYPOINT ["/usr/local/bin/anchor"]
RUN apt update && apt dist-upgrade -y && apt install -y --no-install-recommends \
  libssl-dev \
  ca-certificates \
  && apt-get clean \
  && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/anchor /usr/local/bin/anchor
