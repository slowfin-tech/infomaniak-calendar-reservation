# SAV Appointment Server - Documentation

## Overview

Server Rust pour la gestion des créneaux de rendez-vous avec le service après-vente (SAV).

**Objectif** : Savoir quels sont les slots **restant disponibles** et permettre d'en **réserver un**.

### Fonctionnalités

- Génération automatique des slots disponibles selon les horaires d'ouverture
- Filtrage des slots déjà réservés
- API REST pour consulter les slots DISPONIBLES et réserver un créneau
- Réservation possible uniquement **2 jours à l'avance minimum**
- Configuration via fichier `.env` pour l'URL du calendrier Infomaniak
- Intégration future avec kcalendar et kmeet d'Infomaniak

## Configuration

Créer un fichier `.env` à la racine du projet :

```bash
cp .env.example .env
```

### Variables d'environnement

| Variable | Description | Obligatoire | Valeur par défaut |
|----------|-------------|------------|------------------|
| `KCALENDAR_URL` | URL du calendrier Infomaniak (kcalendar) pour récupérer les disponibilités | Non | - |
| `VISIO_BASE_URL` | URL de base pour générer les liens visio (kmeet) | Non | `https://meet.kmeet.infomaniak.com/sav` |

**Exemple de fichier `.env`** :
```
# URL du calendrier Infomaniak
KCALENDAR_URL=https://kcalendar.infomaniak.com/cal/xxxxxxxxxx/calendar.ics

# URL de base pour les liens visio
VISIO_BASE_URL=https://meet.kmeet.infomaniak.com/sav
```

> **Note** : Le fichier `.env` est ignoré par git (ajouté au `.gitignore`).

## Spécifications des Slots

### Jours d'ouverture
- **Lundi**
- **Mardi**
- **Jeudi**
- **Vendredi**

*Fermé le mercredi, samedi et dimanche.*

### Horaires
- **Matin**: 09h30 - 12h00
  - Créneaux: 09h30-10h00, 10h00-10h30, 10h30-11h00, 11h00-11h30, 11h30-12h00
  - **Total: 5 slots**
- **Après-midi**: 14h00 - 16h00
  - Créneaux: 14h00-14h30, 14h30-15h00, 15h00-15h30, 15h30-16h00
  - **Total: 4 slots**

**Total par jour ouvert: 9 slots de 30 minutes**

### Structure d'un Slot

```json
{
  "id": "202501200930",
  "start_time": "09:30",
  "end_time": "10:00",
  "booked": false
}
```

**Note** : Le champ `date` a été supprimé du slot car il est déjà présent au niveau du `day` parent.

## Règles de Réservation

- **Minimum 2 jours à l'avance** : Un slot ne peut être réservé que s'il est au moins 2 jours dans le futur
- **Un seul slot à la fois** : Chaque slot a un identifiant unique
- **Pas d'annulation** : Pour l'instant, une réservation ne peut pas être annulée via l'API

## API REST

### Endpoints

#### GET /api/health
Vérifie que le serveur est opérationnel.

**Réponse**
```json
{
  "status": "ok",
  "message": "SAV Server is running"
}
```

#### GET /api/slots
Récupère **tous les slots** (disponibles et réservés) pour une plage de dates.

**Query Parameters**
- `start` (optionnel): Date de début au format `YYYY-MM-DD`. Par défaut: aujourd'hui + 2 jours
- `end` (optionnel): Date de fin au format `YYYY-MM-DD`. Par défaut: `start + 7 jours`

**Exemple de requête**
```
GET /api/slots?start=2025-01-22&end=2025-01-24
```

**Réponse**
```json
{
  "days": [
    {
      "date": "2025-01-22",
      "slots": [
        {
          "id": "202501220930",
          "start_time": "09:30",
          "end_time": "10:00",
          "booked": false
        },
        {
          "id": "202501221000",
          "start_time": "10:00",
          "end_time": "10:30",
          "booked": false
        }
      ]
    },
    {
      "date": "2025-01-23",
      "slots": [...]
    },
    {
      "date": "2025-01-24",
      "slots": [...]
    }
  ]
}
```

#### GET /api/slots/{date}
Récupère **tous les slots** (disponibles et réservés) pour une date spécifique.

**Exemple de requête**
```
GET /api/slots/2025-01-22
```

**Réponse**
```json
{
  "days": [
    {
      "date": "2025-01-22",
      "slots": [
        {
          "id": "202501220930",
          "start_time": "09:30",
          "end_time": "10:00",
          "booked": false
        },
        {
          "id": "202501221000",
          "start_time": "10:00",
          "end_time": "10:30",
          "booked": false
        }
      ]
    }
  ]
}
```

#### POST /api/slots/{slot_id}/book
Réserve un slot spécifique.

**Exemple de requête**
```
POST /api/slots/202501220930/book
```

**Requête avec nom client**
```
POST /api/slots/202501220930/book
Content-Type: application/json

{
  "customer_name": "John Doe",
  "customer_email": "john@example.com"
}
```

**Réponse (succès)**
```json
{
  "status": "booked",
  "slot_id": "202501220930",
  "kmeet_url": "https://meet.infomaniak.com/room/abc123",
  "message": "Slot successfully booked"
}
```

**Réponse (échec)**
```json
{
  "error": "Slot cannot be booked. It may already be booked or the date is too close (minimum 2 days in advance)"
}
```

#### GET /
Endpoint racine avec la documentation de l'API et la configuration actuelle.

**Réponse**
```json
{
  "name": "SAV Appointment Server",
  "version": "0.2.0",
  "description": "API for managing SAV appointment slots - Reservation only",
  "endpoints": {...},
  "booking_rules": {
    "minimum_advance_days": 2
  },
  "configuration": {
    "kcalendar_url": "https://kcalendar.infomaniak.com/cal/...",
    "visio_base_url": "https://meet.kmeet.infomaniak.com/sav"
  }
}
```

## Architecture du Projet

```
sav/
├── .env                 # Configuration locale (ignoré par git)
├── .env.example         # Exemple de configuration
├── .gitignore
├── Cargo.toml           # Dépendances
├── DOCUMENTATION.md     # Documentation
└── src/
    └── main.rs          # Code source principal
```

### Composants

1. **Models**: Structure `Slot` avec champ `booked: bool`
2. **AppState**: État partagé avec `HashMap<String, bool>` pour les réservations
3. **Configuration**: Chargement via `dotenvy`, fonctions `get_kcalendar_url()` et `get_visio_base_url()`
4. **Slots Logic**: Génération et filtrage des créneaux disponibles
5. **API Endpoints**: 4 endpoints REST + 1 endpoint de documentation
6. **Tests**: 8 tests unitaires

### Stockage

- **Réservations**: En mémoire (HashMap + Mutex) - perdues au redémarrage
- **Configuration**: Fichier `.env` chargé au démarrage

## Technologie

- **Langage**: Rust
- **Framework Web**: Actix-web
- **Gestion des dates**: Chrono
- **Sérialisation**: Serde
- **Configuration**: dotenvy
- **Stockage**: HashMap + Mutex (en mémoire)

## Dépendances

```toml
[dependencies]
actix-web = "4"
tokio = { version = "1", features = ["full"] }
chrono = "0.4"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
dotenvy = "0.15"
```

## Lancement du Serveur

```bash
# En mode développement
cargo run

# En mode production (release)
cargo run --release

# Vérification de la compilation
cargo check

# Exécution des tests
cargo test
```

Le serveur sera accessible sur **http://localhost:8080**

Au démarrage, le serveur affiche la configuration chargée :
```
SAV Server starting on http://localhost:8080
KCalendar URL: Some("https://kcalendar.infomaniak.com/...")
Visio Base URL: https://meet.kmeet.infomaniak.com/sav
```

## Tests Unitaires

8 tests sont inclus dans le projet :

1. `test_slot_creation` - Vérifie la création d'un slot avec le champ `booked`
2. `test_generate_slots_for_monday` - Vérifie la génération des slots pour un lundi
3. `test_generate_slots_for_saturday` - Vérifie qu'aucun slot n'est généré le samedi
4. `test_generate_slots_for_wednesday` - Vérifie qu'aucun slot n'est généré le mercredi
5. `test_generate_slots_for_range` - Vérifie la génération sur une plage de dates
6. `test_visio_link_format` - Vérifie le format des liens visio
7. `test_app_state_book_slot` - Vérifie la réservation d'un slot dans AppState
8. `test_filter_available_slots` - Vérifie le filtrage des slots réservés

**Exécution**
```bash
cargo test
```

## Historique des Versions

### v0.3.0
- Ajout du fichier `.env` pour la configuration
- Ajout de la dépendance `dotenvy` pour charger les variables d'environnement
- Configuration de `KCALENDAR_URL` pour récupérer le calendrier
- Configuration de `VISIO_BASE_URL` pour personnaliser les liens visio
- Affichage de la configuration au démarrage
- Mise à jour de la documentation

### v0.2.0
- Suppression de l'endpoint `/api/slots/today` (réservation minimum 2 jours à l'avance)
- Ajout du champ `booked` à la structure `Slot`
- Ajout de l'état partagé `AppState` pour gérer les réservations
- Ajout de l'endpoint POST `/api/slots/{slot_id}/book` pour réserver un slot
- Filtrage des slots réservés dans les endpoints GET
- Ajout de la règle : réservation minimum 2 jours à l'avance
- 8 tests unitaires

### v0.1.0 (Initial)
- Création du projet Rust
- Implémentation de la structure `Slot`
- Génération des créneaux selon les horaires spécifiés
- API REST avec endpoints de consultation
- 6 tests unitaires
- Documentation markdown

## Prochaines Étapes

1. **Intégration avec kcalendar**
   - Utiliser `KCALENDAR_URL` pour récupérer les disponibilités réelles
   - Synchronisation avec le calendrier Infomaniak
   - Vérification des conflits

2. **Persistence des réservations**
   - Ajouter un système de stockage (fichier, base de données)
   - Sauvegarder les réservations entre les redémarrages

3. **Intégration avec kmeet**
   - Génération réelle des liens visio
   - Gestion des salles de réunion

4. **Améliorations possibles**
   - Ajout d'un endpoint pour annuler une réservation
   - Gestion des utilisateurs et authentification
   - Notification par email lors de la réservation
   - Interface d'administration
   - Historique des réservations

## Notes

- Les liens visio sont actuellement des placeholders et suivent le format : `{VISIO_BASE_URL}/{date}/{time}`
- La variable `KCALENDAR_URL` est prête pour une intégration future avec kcalendar
- Les réservations sont actuellement stockées en mémoire (perdues au redémarrage)
- La règle des **2 jours minimum** est strictement appliquée
