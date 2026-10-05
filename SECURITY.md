# Politique de sécurité

## Versions prises en charge

Projet en version initiale (`0.1.x`) : seule la dernière version de la branche
principale est prise en charge.

## Signaler une vulnérabilité

**N'ouvrez pas d'issue publique pour une faille de sécurité.**

Contactez les mainteneurs en privé (adresse de contact à renseigner par
l'équipe). Décrivez :

- le composant concerné (serveur, module wasm, initializer JS, image Docker)
- les étapes pour reproduire ou une preuve de concept
- l'impact estimé

Un accusé de réception est attendu sous quelques jours ; le traitement et la
publication d'un correctif seront coordonnés avec vous, avec crédit dans le
`CHANGELOG.md` sauf demande contraire.

## Points d'attention connus

- **Secrets** : tout secret vit dans `.env` (non versionné) ou l'environnement
  du conteneur — jamais dans le dépôt, l'image Docker embarque `config.toml`
  (non-secret) uniquement
- **Clé d'API du module wasm** : l'interface wasm embarque la clé d'API au
  build ; elle est par nature lisible dans le `.wasm`. Elle doit rester
  dédiée à ce frontend et révocable indépendamment des autres secrets
- **Email** : l'envoi SMTP se fait en STARTTLS (587) ou TLS implicite (465)
- Si vous découvrez une fuite de secret dans l'historique : signalez-le
  immédiatement et faites **révoquer** le secret concerné avant toute
  correction
