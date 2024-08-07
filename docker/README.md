To build a statically linked server image:
```
docker build .. -t crusader -f server-static.Dockerfile
```

Supported platforms:
- `linux/i386`
- `linux/x86_64`
- `linux/arm/v7`
- `linux/arm64`

Available profiles:
 - `--build-arg PROFILE=release` (default)
 - `--build-arg PROFILE=speed`
 - `--build-arg PROFILE=size`