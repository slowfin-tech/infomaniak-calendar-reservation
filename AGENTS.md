# SAV - CalDAV Events Server

## Description

Serveur Rust utilisant Actix-web pour exposer une API REST qui récupère les événements calendar depuis Infomaniak via CalDAV.

## Structure du projet

```
sav/
├── Cargo.toml           # Dépendances du projet
├── AGENTS.md            # Ce fichier - documentation
└── src/
    ├── main.rs          # Point d'entrée - serveur Actix
    ├── booking.rs       # Module réservation - POST /api/bookings, API Infomaniak kMeet
    ├── calendar.rs       # Module CalDAV - logique métier
    └── slots.rs          # Module slots - créneaux de SAV de 30 min
```

---

## Configuration

### Variables d'environnement requises

| Variable | Description | Exemple |
|----------|-------------|---------|
| `CALDAV_URL` | URL base du serveur CalDAV | `https://sync.infomaniak.com/` |
| `CALDAV_CALENDAR_URI` | Chemin du calendar | `/caldav/calendar/abc123/` |
| `CALDAV_LOGIN` | Identifiant de connexion | `user@domain.com` |
| `CALDAV_PASSWORD` | Mot de passe | `********` |
| `SAV_API_KEY` | Clé d'API exigée sur tous les endpoints (`Authorization: Bearer <clé>`) | hex aléatoire |
| `KMEET_API_TOKEN` | Token Bearer pour `api.infomaniak.com` | `********` |
| `KCALENDAR_ID` | ID du calendar Infomaniak où créer l'événement | `2438` |
| `VISIO_BASE_URL` | Hostname kMeet | `kmeet.infomaniak.com` |

**Optionnel :** `CALDAV_URL` a une valeur par défaut : `https://sync.infomaniak.com/`

---

## Sécurité

- **Authentification par clé d'API** : tous les endpoints exigent le header `Authorization: Bearer $SAV_API_KEY`. Sans clé valide : `401 Unauthorized`. La clé est destinée au backend appelant (l'utilisateur final est authentifié côté application, il ne saisit pas son email).
- **Rate limiting** : `actix-governor`, par IP : 1 requête autorisée toutes les 10 secondes, burst de 50. Au-delà : `429 Too Many Requests`.
- **HTTPS** : à terminer au reverse proxy (Caddy/nginx) devant le serveur ; actix écoute en HTTP.

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
- Durée fixe de 30 minutes
- ID au format `YYYYMMDDHHMM` (début du créneau)
- `start_at` au format `HH:MM` (heure locale de début du créneau)
- Propriété `booked` (booléen) — `true` si un événement du calendar chevauche le créneau

**Plages horaires quotidiennes :** 10:00–12:00 et 14:00–16:00
(soit 8 slots/jour : 10:00, 10:30, 11:00, 11:30, 14:00, 14:30, 15:00, 15:30)

**Règles :**
- Les slots sont groupés par date (clé `YYYY-MM-DD`), dans l'ordre chronologique
- Un slot chevauchant un événement du calendar est marqué `booked: true`
- Les slots déjà passés (avant l'heure courante) ne sont pas retournés
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
  "email": "client@domain.com",
  "description": "Probleme de connexion au boitier",
  "slot_id": "202609291430"
}
```

**Validation (400 Bad Request) :**
- `slot_id` au format `YYYYMMDDHHMM`
- `slot_id` doit être un début de créneau valide (grille 30 min, 10h–12h / 14h–16h)
- `slot_id` dans la semaine glissante et dans le futur
- `email` non vide et contenant un `@`

**Autres codes :**
- `409 Conflict` : slot déjà réservé (chevauchement avec un événement du calendar)
- `502 Bad Gateway` : erreur de l'API Infomaniak
- `201 Created` : réservation créée

**Réponse 201 :**
```json
{
  "slot_id": "202609291430",
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
- L'événement est créé avec le titre `SAV - {email}`, la description `{description}\n\nContact: {email}`, timezone `Europe/Zurich`
- Anti double-réservation : le `slot_id` est « claimé » atomiquement en mémoire (`Mutex<HashSet>`) pendant la vérification CalDAV + la création de l'événement ; une requête concurrente sur le même slot reçoit `409` immédiatement
- Limite du verrou : il est en mémoire du process, donc valable pour une instance unique du serveur. Pour plusieurs instances (ou un redémarrage), il faudrait un verrou partagé (Redis, ou table SQL avec contrainte d'unicité sur `slot_id`)

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
# Démarrer le serveur
cargo run

# Le serveur écoute sur :0.0.0.0:8080
```

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
