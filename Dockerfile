# syntax=docker/dockerfile:1
# Builds mnemoa (CLI) and mnemoa-api with the optional LLM drafter compiled in.
# It stays off until MNEMOA_LLM_ENABLED=true.
ARG RUST_IMAGE=rust:1.97-slim-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim

FROM ${RUST_IMAGE} AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake clang pkg-config git \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p mnemoa-cli -p mnemoa-api --features mnemoa-cli/llm,mnemoa-api/llm

FROM ${RUNTIME_IMAGE}
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git python3 python3-pip curl \
    && python3 -m pip install --no-cache-dir --break-system-packages ast-grep-cli \
    && apt-get purge -y python3-pip && apt-get autoremove -y \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 mnemoa
COPY --from=build /src/target/release/mnemoa /src/target/release/mnemoa-api /usr/local/bin/
# Repos mounted into /workspace are owned by the host user; let git read them.
RUN git config --system --add safe.directory '*'
USER mnemoa
ENV MNEMOA_ROOT=/workspace \
    MNEMOA_PORT=8080 \
    RUST_LOG=info
WORKDIR /workspace
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --retries=3 CMD curl -fsS http://localhost:8080/health || exit 1
CMD ["mnemoa-api"]
