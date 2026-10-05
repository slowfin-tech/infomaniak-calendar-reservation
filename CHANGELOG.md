# Changelog

Les changements notables de ce projet sont documentés ici.
Format inspiré de [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/),
versionnement [SemVer](https://semver.org/lang/fr/).

## [Non publié]

### Ajouté

- Serveur SAV (Actix-web) : `GET /api/calendar/events` (événements CalDAV
  de la semaine glissante, expansion des récurrences RRULE côté client)
- `GET /api/slots` : créneaux de 30 min dérivés du calendar, groupés par
  date, statut `booked` par chevauchement
- `POST /api/bookings` : réservation via l'API kMeet « Plan a conference »
  (événement calendar + salle visio + participant), validation du créneau,
  anti double-réservation par verrou mémoire, refus si un rendez-vous
  existe déjà pour l'email
- `POST /api/bookings/cancel` : annulation (suppression de l'événement via
  CalDAV), avec confirmation côté UI
- `GET /api/bookings/next` : prochain rendez-vous de l'utilisateur, lien de
  la salle visio (propriété `X-INFOMANIAK-MEET-ROOM-URL`)
- Configuration centralisée : `config.toml` (port, CalDAV, Infomaniak, grille
  des créneaux — durée et périodes par jour, délai minimum, modèles de
  description et d'email, textes UI), secrets dans `.env`
- Email de confirmation SMTP (STARTTLS/TLS) avec pièce jointe `.ics`
  (« ajouter à son agenda »)
- Interface wasm (Yew) : calendrier des créneaux (libres/réservés, pause du
  midi matérialisée), formulaire de réservation, vue « votre rendez-vous »
  avec lien visio et annulation — affichage en modal
- Initializer type Google Analytics (`sav.js`) : injection du CSS, file
  d'attente de commandes, chargement du module wasm au clic uniquement
- Sécurité : authentification par clé d'API Bearer, rate limiting par IP
- Docker : image multi-stage alpine (binaire statique musl) servant l'API
  et le bundle UI sur une seule origine
- Tests : serveur (`cargo test`), module wasm (`wasm-pack test --node`),
  smoke test du module contre le serveur réel
