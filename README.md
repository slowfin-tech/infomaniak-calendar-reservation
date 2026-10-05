# SAV - Réservation de créneaux

Serveur Rust (Actix-web) + interface WebAssembly (Yew) pour gérer des
créneaux de rendez-vous SAV : les créneaux sont dérivés d'un calendar
Infomaniak (CalDAV), la réservation crée l'événement avec sa salle visio
kMeet et envoie un email de confirmation (pièce jointe agenda incluse).

## Fonctionnalités

- **Créneaux dérivés du calendar** — `GET /api/slots` liste les créneaux de la
  semaine glissante, marqués `booked` lorsqu'un événement chevauche
- **Grille configurable** — durée des créneaux et périodes par jour de
  semaine (`config.toml`), délai minimum avant réservation
- **Réservation** — `POST /api/bookings` crée l'événement sur le calendar
  Infomaniak via l'API kMeet « Plan a conference » (salle visio + invitation
  au participant), anti double-réservation
- **Annulation** — `POST /api/bookings/cancel` supprime l'événement via
  CalDAV ; le prochain rendez-vous d'un utilisateur est consultable via
  `GET /api/bookings/next` (avec le lien de la salle visio)
- **Email de confirmation** — envoi SMTP (STARTTLS/TLS), corps configurable
  par modèles, pièce jointe `.ics` pour ajouter le rendez-vous à son agenda
- **Interface wasm** — application Yew compilée en WebAssembly, intégrable
  dans n'importe quelle page via un initializer type Google Analytics ;
  le module n'est chargé qu'au clic, affichage en modal
- **Déploiement** — image Docker multi-stage (binaire statique musl, ~30 Mo)
  servant l'API et le bundle UI sur une seule origine

## Architecture

```
sav/
├── srv/            # Serveur HTTP (Actix-web) : creneaux, réservations, emails
├── ui/             # Interface wasm (Yew) + initializer JS + bundle statique
├── config.toml     # Configuration non-secret
├── .env            # Secrets (non versionné)
├── Dockerfile      # Image serveur + UI
└── Makefile        # Raccourcis build/test/run
```

Les deux composants ne partagent aucun code : l'UI communique uniquement via
l'API HTTP du serveur. Documentation détaillée : [`AGENTS.md`](AGENTS.md)
(serveur) et [`ui/README.md`](ui/README.md) (interface).

## Démarrage rapide

```bash
# 1. Secrets
cp .env.example .env   # puis renseigner CALDAV_*, KMEET_API_TOKEN, SAV_API_KEY, SMTP_*

# 2. En local
make run               # serveur sur http://localhost:8080
make ui-build          # module wasm + bundle statique
python3 -m http.server 8000 --directory ui   # dev UI (page de démo)

# 3. Ou en conteneur (API + UI sur une seule origine)
make ui-build
docker build -t sav-server .
docker run --rm -p 8080:8080 --env-file .env sav-server
# -> http://localhost:8080/
```

## API

| Endpoint | Description |
|---|---|
| `GET /api/calendar/events` | Événements du calendar pour la semaine glissante |
| `GET /api/slots` | Créneaux de la semaine (libres et réservés), par date |
| `POST /api/bookings` | Réserve un créneau (événement + visio + email) |
| `POST /api/bookings/cancel` | Annule le prochain rendez-vous d'un utilisateur |
| `GET /api/bookings/next?email=` | Prochain rendez-vous (date, heure, lien visio) |

Tous les endpoints exigent `Authorization: Bearer $SAV_API_KEY`. Détail des
requêtes/réponses : [`AGENTS.md`](AGENTS.md).

## Configuration

- **`config.toml`** — port, CalDAV, calendar Infomaniak, grille des créneaux,
  modèles de description/email, textes de l'interface (voir le fichier, abondamment commenté)
- **`.env`** — secrets uniquement : identifiants CalDAV, token kMeet, clé
  d'API, identifiants SMTP

## Tests

```bash
make test           # tests du serveur (cargo test)
make ui-test        # tests du module wasm (wasm-pack test --node)
make smoke          # test manuel du module contre le serveur réel
```

## Contribuer

Voir [`CONTRIBUTING.md`](CONTRIBUTING.md). Signaler un problème de sécurité :
[`SECURITY.md`](SECURITY.md).

## Licence

À définir — voir [`LICENSE`](LICENSE).
