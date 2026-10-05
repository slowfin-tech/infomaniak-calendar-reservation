// Smoke test du module wasm sav_ui (application Yew) contre le serveur SAV reel.
// Aucun navigateur requis: le module pkg/sav_ui.js est charge dans Node.
//
//   node ui/scripts/smoke.mjs                    # liste les creneaux
//   node ui/scripts/smoke.mjs --book 202610011000 --email client@domain.com
//
// La cle d'API et l'URL du serveur sont celles du build (build.rs lit le .env
// du repo ou les variables SAV_API_KEY / SAV_URL).

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import init, * as mod from "../pkg/sav_ui.js";

// reqwest/wasm attend un global `window`, absent de Node.
globalThis.window = globalThis;

// Le glue --target web charge le .wasm par fetch relatif: sous Node on fournit
// directement les octets.
const wasmBytes = await readFile(new URL("../pkg/sav_ui_bg.wasm", import.meta.url));
await init({ module_or_path: wasmBytes });

const args = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(`--${name}`);
  return i !== -1 ? args[i + 1] : undefined;
};

try {
  if (args.includes("--book")) {
    const slotId = flag("book");
    const email = flag("email");
    if (!slotId || !email) {
      console.error("--book <slot_id> --email <email> requis (--name et --description optionnels)");
      process.exit(1);
    }
    const booking = await mod.smoke_book(
      flag("name") || email,
      email,
      flag("description") || "Test manuel",
      slotId
    );
    console.log("Reservation OK:");
    console.log(`  slot_id: ${booking.slot_id}`);
    console.log(`  start:   ${booking.start}`);
    console.log(`  end:     ${booking.end}`);
    console.log(`  event:   ${JSON.stringify(booking.event)}`);
  } else {
    const slots = await mod.smoke_list_slots();
    let free = 0;
    let booked = 0;
    for (const [date, list] of Object.entries(slots)) {
      if (list.length === 0) continue;
      const rendered = list
        .map((s) => (s.booked ? `${s.start_at} [pris]` : s.start_at))
        .join(" ");
      console.log(`${date}: ${rendered}`);
      free += list.filter((s) => !s.booked).length;
      booked += list.filter((s) => s.booked).length;
    }
    console.log(`\n${free} creneaux disponibles, ${booked} deja pris`);
  }
} catch (err) {
  console.error(`ECHEC: ${err}`);
  process.exit(1);
}
