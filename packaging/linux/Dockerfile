FROM --platform=linux/amd64 ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential pkg-config curl ca-certificates file dpkg-dev \
    libasound2-dev libx11-dev libxcb1-dev libxkbcommon-dev libwayland-dev \
    libxcursor-dev libxrandr-dev libxi-dev libxinerama-dev \
    libfontconfig1-dev libfreetype6-dev \
 && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain stable
ENV PATH=/root/.cargo/bin:$PATH
WORKDIR /src
