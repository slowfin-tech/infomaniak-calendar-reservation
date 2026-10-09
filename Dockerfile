# Build du serveur infomaniak-calendar-reservation dans un conteneur.
# Image: infomaniak-calendar-reservation  |  Port: 8080 (config.toml)
# Config: variables d'environnement (-e ou --env-file)
#
# Alpine + musl: le binaire est statique (rustls, pas d'OpenSSL systeme),
# l'image runtime ne contient que le binaire + les certificats CA.

# --- Etape 1 : build release (musl) ---
FROM rust:1-alpine AS builder
WORKDIR /app

# musl-dev/gcc: compilation des crates avec code natif (ring/rustls).
RUN apk add --no-cache musl-dev gcc

# Manifestes d'abord pour beneficier du cache Docker sur les dependances.
COPY Cargo.toml Cargo.lock ./
# Le workspace reference ui/: le manifeste doit etre present pour que cargo resolve.
COPY ui/Cargo.toml ui/Cargo.toml

# Sources factices pour compiler les dependances sans le code applicatif
# (ui/src est requis pour que cargo resolve le workspace).
RUN mkdir -p srv ui/src && echo "fn main() {}" > srv/main.rs && echo "" > ui/src/lib.rs
RUN cargo build --release && rm -rf srv target/release/deps/infomaniak*

# Code reel : seul ce layer est recompile quand le code change.
COPY srv ./srv
RUN touch srv/main.rs && cargo build --release

# --- Etape 2 : image d'execution minimale ---
FROM alpine:3.20
WORKDIR /app

# Certificats CA pour les appels HTTPS (Infomaniak). Le binaire est statique.
RUN apk add --no-cache ca-certificates

# Configuration non-secret (config.toml); les secrets restent dans l'env.
COPY config.toml /app/config.toml

# Bundle UI: infomaniak.js (initializer), infomaniak.css, module wasm (pkg/)
# et page de demo. Le module wasm doit etre construit avant (make ui-build) -
# voir ui/README.md.
COPY ui/static /app/ui/static

COPY --from=builder /app/target/release/infomaniak-calendar-reservation /usr/local/bin/infomaniak-calendar-reservation

# Pas de .env dans l'image (secrets): la configuration passe par l'env.
# CALDAV_LOGIN, CALDAV_PASSWORD, KMEET_API_TOKEN, INFOMANIAK_API_KEY (ou
# SAV_API_KEY legacy), SMTP_USER, SMTP_PASSWORD, SMTP_SEND_URL, SMTP_SEND_PORT.
# Le port vient de config.toml ([server] port, defaut 8080).
EXPOSE 8080

CMD ["infomaniak-calendar-reservation"]
