# Contribuer

Merci de l'intérêt pour ce projet. Voici comment démarrer.

## Développement local

```bash
git clone <repo> && cd sav
cp .env.example .env      # renseigner les secrets (voir AGENTS.md)
make run                  # serveur sur :8080
```

Outils requis : Rust (stable), Docker, `wasm-pack` (`cargo install wasm-pack`),
Node 18+ (pour les tests du module wasm et le script de smoke test).

## Structure du projet

```
srv/     # serveur HTTP (Actix-web) : modules main, config, calendar, slots,
         # booking, email
ui/      # crate wasm (Yew) + static/ (initializer sav.js, CSS, bundle)
```

Les deux crates ne partagent **aucun code** : l'UI dialogue avec le serveur
uniquement via son API HTTP. Toute évolution de l'API doit donc rester
rétro-compatible ou être documentée dans `CHANGELOG.md` et `AGENTS.md`.

## Avant de proposer une modification

- `make test` — les tests du serveur doivent passer
- `make ui-build && make ui-test` — le module wasm compile et ses tests passent
- Pas de warning de compilation (`cargo build` doit être propre)
- Les secrets ne vont jamais dans `config.toml`, le code ou la doc : seulement
  dans `.env` (non versionné) ou l'environnement

## Conventions

- Le code, les messages de commit et la documentation sont en français
  (libellés de l'UI compris)
- La configuration non-secret vit dans `config.toml` ; documenter toute
  nouvelle clé dans `AGENTS.md` (et dans le fichier lui-même)
- Les nouvelles fonctionnalités de l'UI doivent rester configurables
  (section `[ui]` de `config.toml`, injectée au build)
- Un commit par intention, message impératif court (`feat: ...`, `fix: ...`,
  `docs: ...`, `refactor: ...`)

## Signaler un bug / proposer une fonctionnalité

Ouvrir une issue avec :
- le contexte (local, conteneur, navigateur concerné)
- les étapes de reproduction
- ce qui était attendu vs. observé

Pour les vulnérabilités, ne pas ouvrir d'issue publique : voir
[`SECURITY.md`](SECURITY.md).
