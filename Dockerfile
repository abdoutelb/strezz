# Both stages use the same Debian release. A binary built against one release's
# C library is not guaranteed to start on an older one.
FROM rust:1-slim-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY web web
RUN cargo build --release

FROM debian:trixie-slim
COPY --from=build /src/target/release/strezz /usr/local/bin/strezz
EXPOSE 4000
# Coordinator by default; override the command to run a worker:
#   docker run strezz worker --coordinator http://<ip>:4000 --token <token>
ENTRYPOINT ["strezz"]
CMD ["coordinator"]
