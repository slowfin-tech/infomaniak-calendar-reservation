# sav-ui

Interface de réservation de créneaux Infomaniak : application **Yew** compilée en
wasm. Membre `ui/` du monorepo, mais **composant indépendant** — la crate ne
partage aucun code avec le serveur et communique uniquement via son API HTTP
(`/api/slots`, `/api/bookings`).

## Configuration au démarrage (runtime)

Aucune configuration n'est compilée dans le module wasm. Au démarrage, le
module charge `GET /api/config` sur le serveur, qui lui fournit :

- `api_key` — la clé d'API (lue depuis `SAV_API_KEY` du `.env` du serveur)
- `ui` — la section `[ui]` de `config.toml` telle quelle (title,
  placeholder, description_label, cancel_confirm, ... : **toute clé ajoutée
  est servie automatiquement**)

L'URL du serveur est l'origine de la page par défaut (déploiement
mono-origine, ex: conteneur Docker). Pour un API sur une autre origine,
passez `apiUrl` à l'initialisation (voir ci-dessous).

La page n'a donc rien à configurer. Attention : la clé est lisible dans le
`.wasm` — elle doit rester dédiée à ce module et révocable. Le titre
(`[ui] title` dans `config.toml`) est rendu par l'application en `<h1>`.

**Le nom et l'email du client sont passés à l'initialisation** (voir ci-dessous)
— les paramètres d'URL (`?name=...&email=...`) restent supportés en repli.

## Structure

```
ui/
├── Cargo.toml      # Crate wasm (sav_ui, cdylib, Yew csr)
├── build.rs        # Injecte SAV_API_KEY / SAV_URL / SAV_TITLE au build
├── src/
│   ├── lib.rs      # Client API + composant Yew + run_app()
│   └── tests.rs    # Tests wasm-bindgen-test
├── static/         # Bundle déployable: sav.js (initialiseur), sav.css, pkg/, démo
├── scripts/        # smoke.mjs : test manuel sans navigateur
└── pkg/            # Sortie wasm-pack (copiée dans static/pkg par make ui-build)
```

## Intégration dans une page hôte (initializer type Google Analytics)

```html
<script>
  window.Sav = window.Sav || function () { (window.Sav.q = window.Sav.q || []).push(arguments); };
  Sav('init', {
    target: '#sav-widget',          // conteneur (défaut: fin du <body>)
    name: 'Jean Dupont',            // nom du client (requis côté API)
    email: 'client@domain.com',     // email du client
    apiUrl: ''                      // optionnel: autre origine pour l'API
  });
</script>
<script async src="https://votre-hote/sav.js"></script>
```

Le chargeur `sav.js` gère tout : file d'attente de commandes (appels avant ou
après son chargement), injection du CSS (`sav.css`), création du bouton et du
conteneur dans `target`, et **chargement du module wasm uniquement au premier
clic**. L'interface (créneaux, formulaire, rendez-vous) s'affiche dans une
**modal avec overlay** : clic sur l'overlay ou sur « × » pour fermer, le
bouton hôte la ré-ouvre sans recharger le module. Le CSS est scopé sous
`.sav-app` — rien ne fuite sur la page hôte.

## Build

```bash
make ui-build    # wasm-pack build + copie du pkg dans static/ (bundle complet)
python3 -m http.server 8000 --directory ui
```

Ouvrir http://localhost:8000/static/ — page de démo qui intègre le widget
exactement comme le ferait une application hôte.

Flux de réservation : les créneaux sont affichés par jour (date au format
« Lundi 5 octobre ») — les **libres** sont cliquables (vert), les
**déjà réservés** apparaissent en orange et ne sont pas cliquables ; un
**clic sur un créneau libre** bascule l'affichage vers un formulaire
demandant la description du problème ; « Confirmer la réservation » appelle
l'API puis revient à la liste rafraîchie, « Annuler » revient sans réserver.
La description est obligatoire.

Si l'utilisateur a déjà un rendez-vous (`GET /api/bookings/next`, tracé par
le titre `SAV - {email}` des événements), **le calendrier n'est pas affiché** :
l'UI montre uniquement « Votre prochain rendez-vous : Lundi 5 octobre à
14:30 » (un seul rendez-vous à la fois — le serveur refuse aussi un second
booking avec `409`), avec un bouton **« Annuler le rendez-vous »**
(`POST /api/bookings/cancel`, suppression de l'événement dans le calendar)
qui ramène ensuite au calendrier des créneaux.

## Tests

### Automatiques

```bash
wasm-pack test --node     # 9 tests, aucun navigateur requis
```

Parsing/filtrage, présence de la config de build, et tests « réseau » via un
stub `fetch` (succès 200/201, 401, 409, URL et body du POST vérifiés, forme
objet JS simple).

### Manuels

**Smoke test en ligne de commande** (serveur Infomaniak démarré) — le vrai module
contre le vrai serveur, sans navigateur :

```bash
node scripts/smoke.mjs
node scripts/smoke.mjs --book 202610011000 --email client@domain.com --description "Probleme"
```

La clé et l'URL sont celles du build. Des raccourcis `make` existent à la
racine du repo : `make run`, `make smoke`, `make ui-serve`, `make ui-test`,
`make ui-build`.
