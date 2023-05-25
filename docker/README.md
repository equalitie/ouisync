# Deployment instructions

## Install

1. Build the image from the project root:

        docker build -t equalitie/ouisync:latest . -f docker/Dockerfile

2. Upload it to the server:

        docker save equalitie/ouisync:latest | bzip2 | pv | ssh <HOST> docker load

    NOTE: `<HOST>` is the hostname of the server to deploy to, `bzip2` is used to compress the
    image during transfer to reduce bandwidth (optional) and `pv` to report transfer progress (also
    optional).

3. Upload `docker-compose.yml` to the server:

        scp docker/docker-compose.yml <HOST>:/some/path

4. If using let's encrypt to handle TLS certificates (recommended), define `DOMAIN` env variable
and set it to the domain name of the server (recommended to use `.env` file next to
`docker-compose.yml`). Otherwise edit the `volumes` section in `docker-compose.yml` and modify the
paths to the certificate and private key accordingly.

5. Log in to the server, cd into the directory where `docker-compose.yml` is and start the
container using docker compose:

        docker compose up -d

## Update

Build and upload the new image as described in the previous section. Then log in to the server and
do:

    docker compose up -d

Optionally prune the old image(s):

    docker image prune

## Control

Control the server by invoking `ouisync` via docker:

    docker exec ouisync ouisync

NOTE: The first `ouisync` is the container name (defined in `docker-compose.yml`) and the second
one is the binary name. It's recommented to create an alias for this (e.g. in `.bashrc`):

    alias ouisync=docker exec ouisync ouisync

