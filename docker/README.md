To build a statically linked server image:
```
docker build .. -t crusader -f server-static.Dockerfile --build-arg TARGET=x86_64-unknown-linux-musl --platform=linux/x86_64
```

Some possible targets:
- `--build-arg TARGET=i686-unknown-linux-musl --platform=linux/i386`
- `--build-arg TARGET=x86_64-unknown-linux-musl --platform=linux/x86_64`
- `--build-arg TARGET=arm-unknown-linux-musleabihf --platform=linux/arm/v7`
- `--build-arg TARGET=aarch64-unknown-linux-musl --platform=linux/arm64`

Available profiles:
 - `--build-arg PROFILE=release` (default)
 - `--build-arg PROFILE=speed`
 - `--build-arg PROFILE=size`