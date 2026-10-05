# SAV - CalDAV Events Server

## Description

Serveur Rust utilisant Actix-web pour exposer une API REST qui récupère les événements calendar depuis Infomaniak via CalDAV.

## Structure du projet

```
sav/
├── Cargo.toml           # Workspace monorepo + crate serveur (sav_server, sources dans srv/)
├── Cargo.lock           # Lockfile du workspace
├── config.toml          # Configuration non-secret (port, caldav, creneaux, titre...)
├── Dockerfile           # Build multi-stage de l'image serveur (sav-server)
├── .dockerignore        # Exclusions du contexte Docker (target/, .env, ui/...)
├── AGENTS.md            # Ce fichier - documentation
├── srv/
│   ├── main.rs          # Point d'entrée - serveur Actix
│   ├── config.rs        # Configuration non-secret (config.toml, global())
│   ├── booking.rs       # Module réservation - POST /api/bookings, API Infomaniak kMeet
│   ├── calendar.rs       # Module CalDAV - logique métier
│   └── slots.rs          # Module slots - créneaux de SAV de 30 min
└── ui/                   # Interface SAV en wasm (crate indépendante sav_ui, voir ui/README.md)
    ├── Cargo.toml        # Crate wasm (cdylib, Yew)
    ├── build.rs          # Injecte SAV_API_KEY / SAV_URL au build (depuis .env ou l'environnement)
    ├── src/lib.rs        # Client API + application Yew
    ├── src/tests.rs      # Tests wasm-bindgen-test (wasm-pack test --node)
    ├── static/           # Bundle déployable: sav.js (initializer type GA), sav.css, pkg/, page de démo
    ├── scripts/smoke.mjs  # Test manuel du module contre le serveur réel
    └── pkg/              # Sortie wasm-pack (générée)
```

Le monorepo contient deux crates qui ne partagent **aucun code** : l'UI
(`ui/`) discute avec le serveur uniquement via son API HTTP. Le build de
l'UI se fait avec `wasm-pack build --target web` depuis `ui/` (voir
`ui/README.md`) ; `cargo build` à la racine ne compile que le serveur
(`default-members`).

---

## Configuration

### `config.toml` — tout ce qui n'est pas secret

Chargé au démarrage (chemin par défaut `config.toml`, surchargeable via `CONFIG_PATH`).
Un fichier absent est toléré (valeurs par défaut) ; un fichier invalide fait échouer le démarrage.

| Section | Clé | Description | Défaut |
|---------|-----|-------------|--------|
| `[server]` | `port` | Port d'écoute | `8080` |
| `[ui]` | `title` | Titre affiché par l'interface (injecté au build du module wasm) | `"SAV - Rendez-vous"` |
| `[ui]` | `placeholder`, `description_label`, `cancel_confirm`, ... | Textes de l'interface — **toute clé de `[ui]` est injectée au build du module wasm** sous la forme `SAV_UI_<CLÉ>` (lue via `option_env!`, défaut dans le code si absente) | défauts dans `ui/src/lib.rs` |
| `[caldav]` | `url` | URL de base du serveur CalDAV | `https://sync.infomaniak.com/` |
| `[caldav]` | `calendar_uri` | Chemin du calendar | — |
| `[infomaniak]` | `calendar_id` | ID du calendar Infomaniak où créer les événements | — |
| `[infomaniak]` | `visio_base_url` | Hostname kMeet pour les liens visio | — |
| `[slots]` | `duration_minutes` | Durée des créneaux en minutes | `30` |
| `[slots]` | `min_delay_minutes` | Délai minimum avant de pouvoir réserver | `0` |
| `[slots.periods]` | `mon`…`sun` | Périodes par jour (`["HH:MM-HH:MM", ...]`) | tous les jours `10:00-12:00` et `14:00-16:00` |
| `[booking]` | `description_template` | Modèle de la description envoyée à kMeet — placeholders `{description}`, `{email}`, `{name}`, `{start}`, `{end}` | `"{description}\n\nContact: {email}"` |
| `[email]` | `sender` | Expéditeur de l'email de confirmation (vide = pas d'email) | — |
| `[email]` | `subject` / `body` | Modèles de l'email — placeholders `{name}`, `{email}`, `{description}`, `{start}`, `{end}`, `{link}` | défauts dans `srv/email.rs` |

**Sémantique de `[slots.periods]`** : si la section est **absente**, périodes par défaut
pour tous les jours ; si elle est **présente**, elle remplace complètement les valeurs
par défaut et les jours non listés sont **fermés** (aucun créneau).

### `.env` — secrets uniquement

| Variable | Description |
|----------|-------------|
| `CALDAV_LOGIN` | Identifiant de connexion CalDAV |
| `CALDAV_PASSWORD` | Mot de passe CalDAV |
| `KMEET_API_TOKEN` | Token Bearer pour `api.infomaniak.com` |
| `SMTP_USER` / `SMTP_PASSWORD` | Identifiants SMTP pour l'email de confirmation |
| `SMTP_SEND_URL` / `SMTP_SEND_PORT` | Hôte et port SMTP (STARTTLS, défaut 587) |
| `SAV_API_KEY` | Clé d'API exigée sur tous les endpoints (`Authorization: Bearer <clé>`) |

---

## Sécurité

- **Authentification par clé d'API** : tous les endpoints exigent le header `Authorization: Bearer $SAV_API_KEY`. Sans clé valide : `401 Unauthorized`. La clé est destinée au backend appelant (l'utilisateur final est authentifié côté application, il ne saisit pas son email).
- **Rate limiting** : `actix-governor`, par IP : 1 requête autorisée toutes les 10 secondes, burst de 50. Au-delà : `429 Too Many Requests`.
- **CORS** : `actix-cors::Cors::permissive()` — le composant UI (crate `ui/` du monorepo, voir son README) appelle l'API depuis une autre origine. L'accès reste protégé par la clé d'API.
- **HTTPS** : à terminer au reverse proxy (Caddy/nginx) devant le serveur ; actix écoute en HTTP.
- **Port** : `SAV_PORT` (défaut `8080`).

---

## API Endpoints

### GET `/api/calendar/events`

**Description :** Récupère les événements pour les **7 prochains jours à partir d'aujourd'hui** (plage glissante).

**Plage de dates :**
- Début : Aujourd'hui (date du jour)
- Fin : Aujourd'hui + 7 jours

**Réponse :**
```json
{
  "events_by_date": {
    "2026-09-24": [
      {
        "summary": "Réunion d'équipe",
        "start": "2026-09-24 14:00:00",
        "end": "2026-09-24 15:30:00"
      }
    ],
    "2026-09-25": [
      {
        "summary": "Rendez-vous client",
        "start": "2026-09-25 10:00:00",
        "end": "2026-09-25 11:00:00"
      }
    ]
  }
}
```

**Format des dates :**
- Clés du HashMap : `YYYY-MM-DD` (format ISO)
- Champs `start`/`end` : `YYYY-MM-DD HH:MM:SS`

---

### GET `/api/slots`

**Description :** Liste **tous** les créneaux de SAV sur la semaine glissante (7 jours), groupés par jour et avec leur statut de réservation.

**Un slot :**
- Durée configurable (`SAV_SLOT_DURATION`, défaut 30 minutes)
- ID au format `YYYYMMDDHHMM` (début du créneau)
- `start_at` au format `HH:MM` (heure locale de début du créneau)
- Propriété `booked` (booléen) — `true` si un événement du calendar chevauche le créneau

**Périodes horaires :** configurables jour par jour via `SAV_PERIODS`
(clés `mon`–`sun`, périodes `"HH:MM-HH:MM"`). Par défaut : 10:00–12:00 et
14:00–16:00 tous les jours. Si `SAV_PERIODS` est défini, il remplace les
valeurs par défaut et les jours non listés sont **fermés** (aucun créneau).
La grille des débuts de créneaux est alignée sur le début de chaque période
(ex. période 09:00–11:45 avec 45 min → 09:00, 09:45, 10:30).

**Règles :**
- Les slots sont groupés par date (clé `YYYY-MM-DD`), dans l'ordre chronologique
- Un slot chevauchant un événement du calendar est marqué `booked: true`
- Les slots déjà passés ou commençant avant `maintenant + SAV_MIN_DELAY_MINUTES` ne sont pas retournés
- Le week-end est inclus pour l'instant

**Réponse :**
```json
{
  "slots_by_date": {
    "2026-09-29": [
      { "id": "202609291400", "start_at": "14:00", "booked": true },
      { "id": "202609291430", "start_at": "14:30", "booked": false },
      { "id": "202609291500", "start_at": "15:00", "booked": false }
    ]
  }
}
```

---

### POST `/api/bookings`

**Description :** Réserve un slot de SAV en créant un événement sur le calendar Infomaniak via l'API « Plan a conference » (`POST https://api.infomaniak.com/1/kmeet/rooms`). Cette API crée une salle visio kMeet **et** l'événement calendar avec l'URL de réunion.

**Requête :**
```json
{
  "name": "Jean Dupont",
  "email": "client@domain.com",
  "description": "Probleme de connexion au boitier",
  "slot_id": "202609291430"
}
```

**Validation (400 Bad Request) :**
- `slot_id` au format `YYYYMMDDHHMM`
- `slot_id` doit être un début de créneau valide selon la configuration (`[slots]` de `config.toml`) pour le jour concerné
- `slot_id` doit respecter le délai minimum (`[slots] min_delay_minutes`) : plus tôt → `400` avec le message « slot_id trop proche »
- `slot_id` dans la semaine glissante et dans le futur
- `name` non vide (obligatoire, comme `email`)
- `email` non vide et contenant un `@`

**Autres codes :**
- `409 Conflict` : slot déjà réservé (chevauchement avec un événement du calendar), réservation en cours sur le même slot, **ou un rendez-vous existe déjà pour cet email** (un seul rendez-vous à la fois par utilisateur)
- `502 Bad Gateway` : erreur de l'API Infomaniak
- `201 Created` : réservation créée

**Réponse 201 :**
```json
{
  "slot_id": "202609291430",
  "name": "Jean Dupont",
  "email": "client@domain.com",
  "description": "Probleme de connexion au boitier",
  "start": "2026-09-29 14:30:00",
  "end": "2026-09-29 15:00:00",
  "event": { "...donnees de l'API Infomaniak (room, event_id)..." }
}
```

**Variables d'environnement requises :**

| Variable | Description |
|----------|-------------|
| `KMEET_API_TOKEN` | Token Bearer pour `api.infomaniak.com` (scopes kmeet + workspace:calendar) |
| `KCALENDAR_ID` | ID du calendar Infomaniak où créer l'événement |
| `VISIO_BASE_URL` | Hostname kMeet (ex: `kmeet.infomaniak.com`) |

**Notes :**
- L'événement est créé avec le titre `SAV - {email}`, la description construite depuis `[booking] description_template` (placeholders `{description}`, `{email}`, `{name}`, `{start}`, `{end}`), timezone `Europe/Zurich`
- Le client est ajouté en participant (`attendees` : `address` = email, `organizer: false`, `name` = nom du client, `state: "NEEDS-ACTION"`) — il reçoit l'invitation
- Un **email de confirmation** est envoyé au client via SMTP (`[email]` de config.toml + secrets SMTP_* du `.env`) : meilleur effort, un échec SMTP est tracé sur stderr mais n'annule pas la réservation
- Anti double-réservation : le `slot_id` est « claimé » atomiquement en mémoire (`Mutex<HashSet>`) pendant la vérification CalDAV + la création de l'événement ; une requête concurrente sur le même slot reçoit `409` immédiatement
- Limite du verrou : il est en mémoire du process, donc valable pour une instance unique du serveur. Pour plusieurs instances (ou un redémarrage), il faudrait un verrou partagé (Redis, ou table SQL avec contrainte d'unicité sur `slot_id`)

---

### GET `/api/bookings/next?email=client@domain.com`

**Description :** Retourne le **prochain rendez-vous** SAV de l'utilisateur (le plus proche événement futur dont le titre est `SAV - {email}`), dans la semaine glissante. C'est ce qui alimente le bandeau de l'UI une fois un rendez-vous pris.

**Réponse (rendez-vous trouvé) :**
```json
{
  "next_booking": {
    "slot_id": "202610031400",
    "start": "2026-10-03 14:00:00",
    "end": "2026-10-03 14:30:00",
    "link": "https://kmeet.infomaniak.com/room123"
  }
}
```

Le champ `link` (lien de la salle visio) est extrait de la description ou du lieu de l'événement calendar ; il est omis si l'événement n'en porte pas. L'UI l'affiche comme bouton « Rejoindre la visio ».

**Réponse (aucun rendez-vous) :** `{ "next_booking": null }`

**Codes :** `400` email invalide, `401` sans clé d'API. La correspondance sur le titre est insensible à la casse.

---

### POST `/api/bookings/cancel`

**Description :** Annule le prochain rendez-vous SAV de l'utilisateur en **supprimant l'événement du calendar Infomaniak** via CalDAV (`DELETE {calendar_uri}/{uid}.ics`).

**Requête :**
```json
{ "email": "client@domain.com" }
```

**Codes :**
- `200` : `{"cancelled": true}` — après un `DELETE` réussi, le serveur revérifie que l'événement a disparu du calendar
- `404` : aucun rendez-vous à annuler
- `400` : email invalide
- `502` : le CalDAV refuse la suppression ou elle n'est pas confirmée

---

## Module calendar.rs

### Structures publiques

```rust
pub struct Event {
    pub summary: String,
    pub start: String,
    pub end: String,
}

pub struct CalendarResponse {
    pub events_by_date: HashMap<String, Vec<Event>>,
}

pub struct CalDavConfig {
    pub url: String,
    pub calendar_uri: String,
    pub login: String,
    pub password: String,
}
```

### Fonctions publiques

| Fonction | Description |
|----------|-------------|
| `CalDavConfig::from_env()` | Crée la config depuis les variables d'environnement |
| `get_sliding_week_range()` | Retourne (aujourd'hui, aujourd'hui + 7 jours) |
| `fetch_caldav_events(config)` | Récupère la réponse brute XML depuis CalDAV |
| `get_sliding_week_calendar_events(config)` | **Fonction principale** - retourne les événements groupés par date |

---

## Fonctionnalités implémentées

### 1. Parsing ICS avec crate `ical`
- Parse les fichiers iCalendar (ICS) retourés par Infomaniak
- Extrait : SUMMARY, DTSTART, DTEND, RRULE

### 2. Gestion des Timezones
- **Les dates Infomaniak incluent `TZID=Europe/Zurich`**
- Les heures sont déjà dans le bon timezone → **pas de conversion UTC→Zurich**
- Affichage direct des heures telles quelles

### 3. Expansion des événements récurrents (RRULE)
Gère les règles de récurrence iCalendar :

| Paramètre | Supporté | Description |
|-----------|----------|-------------|
| `FREQ=DAILY` | ✅ | Événements quotidiens |
| `FREQ=WEEKLY` | ✅ | Événements hebdomadaires |
| `FREQ=MONTHLY` | ✅ | Événements mensuels |
| `FREQ=YEARLY` | ✅ | Événements annuels |
| `INTERVAL=N` | ✅ | Tous les N jours/semaines/mois |
| `UNTIL=date` | ✅ | Limite de récurrence |
| `COUNT=N` | ✅ | Nombre d'occurrences |

**Exemple d'expansion :**
```
DTSTART;TZID=Europe/Zurich:20260911T140000
RRULE:FREQ=DAILY;INTERVAL=1
```
→ Génère une occurrence par jour dans la plage de 7 jours

### 4. Filtrage par plage de dates
- Seuls les événements **dans la plage [aujourd'hui, aujourd'hui+7]** sont retournés
- Les événements récurrents sont **étendus** si leur date de base est avant la plage

---

## Requête CalDAV

La requête utilise la méthode `REPORT` avec un filtre CalDAV :

```xml
<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT"/>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>
```

**Pourquoi pas de time-range ?**
Infomaniak **ne développe pas** les occurrences récurrentes côté serveur dans une plage de dates. 
On récupère donc **tous les événements** puis on filtre et on étend les RRULE côté client.

---

## Dépendances

```toml
[dependencies]
actix-web = "4"
tokio = { version = "1", features = ["full", "rt-multi-thread"] }
chrono = "0.4"
chrono-tz = "0.8"  # Non utilisé actuellement (heures déjà locales)
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
dotenvy = "0.15"
reqwest = { version = "0.11", features = ["json"] }
base64 = "0.21"
ical = "0.11"        # Parsing ICS
```

---

## Exécution

```bash
# En local (dev)
cargo run

# Le serveur écoute sur :0.0.0.0:8080 (configurable via SAV_PORT)
```

## Docker

Build multi-stage : `rust:1-alpine` compile un binaire **statique** en release
(TLS via `rustls`, pas d'OpenSSL système), runtime `alpine:3.20`. Le conteneur
sert **l'API et le bundle UI** (`ui/static` : sav.js, sav.css, module wasm,
page de démo) — une seule origine, pas de CORS nécessaire.

```bash
make ui-build                      # build du module wasm + copie dans ui/static
docker build -t sav-server .       # l'image embarque le bundle
docker run --rm -p 8080:8080 --env-file .env sav-server
```

Ouvrir http://localhost:8080/ (page de démo intégrant le widget).

Notes :
- `.env` (secrets) est exclu du contexte Docker (`.dockerignore`) et passe par `--env-file` au `docker run` ; `config.toml` (non-secret) est copié dans l'image
- Pour surcharger la config en conteneur : monter un `config.toml` et/ou définir `CONFIG_PATH`
- `ui/Cargo.toml` est copié au build car le workspace le référence ; le bundle UI vient de `ui/static` (construit au préalable avec `make ui-build`)
- **Déploiement sur un autre hôte que localhost** : le module wasm a l'URL de l'API cuite au build. Reconstruisez-le avec `SAV_URL=` vide dans le `.env` (`make ui-build`) : le module s'adapte alors à l'origine de la page (mono-origine). Sinon, exportez `SAV_URL=https://votre-hote` avant le build
- Le binaire est statique : aucun paquet runtime à maintenir hors `ca-certificates`
- Contrepartie de musl : allocateur moins performant que glibc sous forte charge, sans impact pour ce serveur

---

## Historique des problèmes résolus

| Problème | Solution |
|----------|----------|
| Réponse vide | Infomaniak ne développe pas les RRULE → expansion côté client |
| Décalage +2h | Les dates ont `TZID=Europe/Zurich` → pas de conversion nécessaire |
| Seulemement 3 événements | Filtre appliqué sur la plage de 7 jours |
| Groupement par jours | Changé pour groupement **par date** (YYYY-MM-DD) |
| Warnings de compilation | Nettoyage des fonctions inutilisées |

---

## Prochaines étapes possibles

1. **Ajouter un endpoint avec plage personnalisée** :
   ```rust
   GET /api/calendar/events?start=2026-09-24&end=2026-10-01
   ```

2. **Pagination** : Pour gérer un grand nombre d'événements

3. **Cache** : Mettre en cache les réponses CalDAV

4. **Gestion d'erreurs améliorée** : Retourner des codes HTTP appropriés

5. **Support de plusieurs calendars** : Récupérer depuis plusieurs calendars Infomaniak

---

## Notes techniques

- **Timezone :** Les dates Infomaniak incluent `TZID=Europe/Zurich`, donc les heures sont déjà locales. Aucune conversion n'est nécessaire.
- **Récurrence :** L'expansion des RRULE est faite manuellement car le crate `recur` n'était pas compatible (pas de lib target).
- **Performance :** La requête CalDAV récupère TOUS les événements, puis filtre côté client. Pour un grand nombre d'événements, envisager une optimisation.
