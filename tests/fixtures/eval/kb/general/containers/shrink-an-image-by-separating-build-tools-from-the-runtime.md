---
schema: 1
id: 2b00000028
type: recipe
status: active
verified: 2026-07-03
verified_how: ran
tags:
  - containers
  - docker
  - multi-stage-build
---

# Shrink an image by separating build tools from the runtime stage

## When to use
When a Dockerfile installs compilers and build dependencies that are not needed once the application binary exists, bloating the final image.

## Steps
```dockerfile
FROM golang:1.22 AS build
WORKDIR /src
COPY . .
RUN go build -o /out/app .

FROM debian:bookworm-slim
COPY --from=build /out/app /usr/local/bin/app
ENTRYPOINT ["/usr/local/bin/app"]
```

Only the compiled artifact is copied into the final stage; the build stage (and its toolchain) is discarded from the resulting image.

## Evidence
The multi-stage build reduced the final image from over 900 MB (with the Go toolchain included) to under 30 MB.
