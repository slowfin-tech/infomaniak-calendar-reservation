// Chargeur du widget Infomaniak - pattern "initializer" a la Google Analytics.
//
// Sur la page hote:
//
//   <script>
//     window.InfomaniakCalendar = window.InfomaniakCalendar || function () { (window.InfomaniakCalendar.q = window.InfomaniakCalendar.q || []).push(arguments); };
//     InfomaniakCalendar('init', { target: '#infomaniak-widget', name: 'Jean Dupont', email: 'client@domain.com' });
//   </script>
//   <script async src="chemin/vers/infomaniak.js"></script>
//
// Le chargeur: injecte le CSS, cree le bouton + le conteneur, et charge le
// module wasm (application Yew) uniquement au clic - jamais a l'ouverture.
// Options de 'init':
//   - target: selecteur CSS du conteneur (defaut: fin du <body>)
//   - name, email: identite du client pour la reservation

(function () {
  "use strict";

  // Base des assets (infomaniak.js etant servi depuis <base>/infomaniak.js).
  var script = document.currentScript;
  var base = new URL(".", script.src).href;

  // File d'attente compatible avec le snippet ci-dessus: les appels faits
  // avant le chargement sont bufferses.
  var queue = (window.InfomaniakCalendar && window.InfomaniakCalendar.q) || [];

  var booted = false;

  function process(args) {
    var command = args[0];
    if (command === "init") {
      boot(args[1] || {});
    }
  }

  function boot(options) {
    if (booted) return;
    booted = true;

    // 1. CSS du widget.
    var link = document.createElement("link");
    link.rel = "stylesheet";
    link.href = new URL("infomaniak.css", base).href;
    document.head.appendChild(link);

    // 2. Conteneur.
    var target = options.target ? document.querySelector(options.target) : document.body;
    if (!target) {
      console.error("[InfomaniakCalendar] cible introuvable:", options.target);
      return;
    }

    var app = document.createElement("div");
    app.className = "infomaniak-app";

    var button = document.createElement("button");
    button.id = "load-slots";
    button.textContent = "Voir les créneaux";

    var root = document.createElement("div");
    root.id = "infomaniak-root";

    app.appendChild(button);
    app.appendChild(root);
    target.appendChild(app);

    // 3. Configuration lisible par le module wasm. La cle d'API et les
    // textes viennent de GET /api/config au demarrage; apiUrl (optionnel)
    // designe un serveur d'API sur une autre origine.
    window.__infomaniak_config = {
      email: options.email || null,
      name: options.name || null,
      apiUrl: options.apiUrl || null,
    };

    // 4. Le wasm n'est charge qu'au premier clic; la modal se re-ouvre via
    // window.__infomaniakToggle aux clics suivants.
    var loaded = false;
    button.addEventListener("click", function () {
      if (loaded) {
        if (window.__infomaniakToggle) window.__infomaniakToggle();
        return;
      }

      button.disabled = true;
      button.textContent = "Chargement...";

      import(new URL("pkg/infomaniak_ui.js", base).href)
        .then(function (mod) { return mod.default().then(function () { return mod; }); })
        .then(function (mod) {
          mod.run_app();
          loaded = true;
          button.disabled = false;
          button.textContent = "Voir les créneaux";
        })
        .catch(function (err) {
          button.disabled = false;
          button.textContent = "Voir les créneaux";
          var message = document.createElement("p");
          message.className = "error";
          message.textContent = "Impossible de charger le module: " + err;
          app.appendChild(message);
        });
    });
  }

  // Drain les commandes bufferisees puis traite les suivantes directement.
  for (var i = 0; i < queue.length; i++) {
    process(queue[i]);
  }
  window.InfomaniakCalendar = function () {
    process(arguments);
  };
})();
