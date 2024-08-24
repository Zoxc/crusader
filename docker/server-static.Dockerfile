FROM rust AS build
ARG TARGETARCH
ARG PROFILE=release

COPY src /src
WORKDIR /src

RUN echo no-target-detected > /target

RUN if [ "$TARGETARCH" = "386" ]; then\
    echo i686-unknown-linux-musl > /target; fi
RUN if [ "$TARGETARCH" = "amd64" ]; then\
    echo x86_64-unknown-linux-musl > /target; fi
RUN if [ "$TARGETARCH" = "arm" ]; then\
    echo arm-unknown-linux-musleabihf > /target; fi
RUN if [ "$TARGETARCH" = "arm64" ]; then\
    echo aarch64-unknown-linux-musl > /target; fi

ENV RUSTFLAGS="-C target-feature=+crt-static"

RUN rustup target add $(cat /target)

RUN cargo build -p crusader --no-default-features --profile=$PROFILE --target $(cat /target)

RUN cp target/$(cat /target)/$PROFILE/crusader /

FROM scratch
COPY --from=build /crusader /

EXPOSE 35481/tcp 35481/udp 35483/udp
ENTRYPOINT [ "/crusader", "serve" ]
