.PHONY: run docker-run ui-build ui-serve ui-test smoke test

# Serveur Infomaniak en local (lit le .env via dotenvy)
run:
	cargo run

# Serveur Infomaniak en conteneur
docker-run:
	docker build -t infomaniak-calendar-reservation .
	docker run --rm -p 8080:8080 --env-file .env infomaniak-calendar-reservation

# Interface wasm: build + bundle deployable dans ui/static/
# (infomaniak.js, infomaniak.css, index.html de demo + pkg/ copie)
ui-build:
	cd ui && wasm-pack build --target web && rm -rf static/pkg && cp -R pkg static/pkg

ui-serve:
	cd ui && python3 -m http.server 8000

# Tests automatiques du module wasm (Node, sans navigateur)
ui-test:
	cd ui && wasm-pack test --node

# Smoke test du module wasm contre le serveur reel (le serveur doit tourner)
smoke:
	node ui/scripts/smoke.mjs

# Tests du serveur
test:
	cargo test
