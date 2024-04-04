# Deployment instructions

## Install

1. Build the image from the project root:

        docker build -t equalitie/ouisync:latest . -f docker/Dockerfile

2. Publish it to dockerhub:

        docker push equalitie/ouisync:latest

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

Build and publish the new image as described in the previous section. Then log in to the server and
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

## Cache server

To setup a cache server, enable the RPC endpoint:

    ouisync bind-rpc 0.0.0.0:443

## Analytics

To setup analytics, copy the `update-geo-ip-db.sh` script to the server and edit to to fill in the
maxmind.com license key. Then run it to download the latest GeoIp database. It's recommended to
update the database periodically, say once every two weeks.

Enable the prometheus metrics endpint:

    ouisync bind-metrics 0.0.0.0:444

Setup prometheus (or any compatible tool) to scrape the endpoint and Grafana (or similar) to
visualize the data.