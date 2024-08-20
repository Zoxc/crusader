To build a statically linked server image:
```
docker build .. -t crusader -f server-static.Dockerfile
```

To build a statically linked remote image:
```
docker build .. -t crusader -f remote-static.Dockerfile
```
This image allow initiation of tests using the web application running on port 35482.

Supported platforms:
- `linux/i386`
- `linux/x86_64`
- `linux/arm/v7`
- `linux/arm64`

Available profiles:
 - `--build-arg PROFILE=release` (default)
 - `--build-arg PROFILE=speed`
 - `--build-arg PROFILE=size`