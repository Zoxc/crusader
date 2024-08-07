ARG TARGET=x86_64-unknown-linux-musl
ARG PROFILE=release

FROM rust AS build
ARG PROFILE
ARG TARGET

COPY src /src
WORKDIR /src

ENV RUSTFLAGS="-C target-feature=+crt-static"

RUN rustup target add $TARGET

RUN cargo fetch
RUN cargo build -p crusader --profile=$PROFILE --target $TARGET

FROM scratch
ARG PROFILE
ARG TARGET
COPY --from=build /src/target/$TARGET/$PROFILE/crusader /crusader

EXPOSE 35481/tcp 35481/udp
ENTRYPOINT [ "/crusader", "serve" ]
