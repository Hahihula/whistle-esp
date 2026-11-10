// Web flasher: writes the firmware image (and optionally the model) with
// esptool-js over Web Serial. The files sit next to this page, put there by
// the release workflow (.github/workflows/release.yml).
import { ESPLoader, Transport } from "https://cdn.jsdelivr.net/npm/esptool-js@0.7.1/bundle.js";

// Flash layout from each board's partitions.csv: the merged image (bootloader,
// partition table, app) starts at 0, the model partition at 0x210000.
const MODEL = { url: "model/whistle_d4.cact", address: 0x210000 };
const BOARDS = {
  vocat: { chip: "ESP32-S3", firmware: "firmware/vocat.bin" },
  korvo: { chip: "ESP32-S31", firmware: "firmware/korvo.bin" },
  p4: { chip: "ESP32-P4", firmware: "firmware/p4.bin" },
};
const BAUD = 921600;

const $ = (id) => document.getElementById(id);
const logEl = $("log");
const terminal = {
  clean() { logEl.textContent = ""; },
  writeLine(line) { logEl.textContent += line + "\n"; },
  write(text) { logEl.textContent += text; },
};

function step(text, percent) {
  $("progress").hidden = false;
  $("step").textContent = text;
  if (percent !== undefined) $("bar").value = percent;
}

function result(text, ok) {
  $("result").textContent = text;
  $("result").className = "result " + (ok ? "ok" : "error");
}

async function download(url, label) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  const total = Number(response.headers.get("content-length")) || 0;
  const reader = response.body.getReader();
  const chunks = [];
  let received = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    received += value.length;
    if (total) step(`Downloading ${label}…`, (100 * received) / total);
  }
  const data = new Uint8Array(received);
  let offset = 0;
  for (const c of chunks) { data.set(c, offset); offset += c.length; }
  return data;
}

async function flash() {
  const board = BOARDS[document.querySelector("input[name=board]:checked").value];
  const withModel = $("with-model").checked;
  $("flash").disabled = true;
  result("", true);
  let transport;
  try {
    // Ask for the port first: the browser only allows it right after a click.
    const port = await navigator.serial.requestPort();

    const files = [{ data: await download(board.firmware, "firmware"), address: 0 }];
    if (withModel) files.push({ data: await download(MODEL.url, "speech model"), address: MODEL.address });

    step("Connecting to the board…", 0);
    transport = new Transport(port, true);
    const loader = new ESPLoader({ transport, baudrate: BAUD, terminal });
    const chip = await loader.main();
    terminal.writeLine(`Connected: ${chip}`);
    // e.g. "ESP32-S3 (QFN56) (revision v0.2)": the first word is the chip.
    if (chip.split(" ")[0] !== board.chip) {
      throw new Error(`This is an ${chip}, not the ${board.chip} the selected board has. Pick another board.`);
    }

    const sizes = files.map((f) => f.data.length);
    const total = sizes.reduce((a, b) => a + b, 0);
    const before = (i) => sizes.slice(0, i).reduce((a, b) => a + b, 0);
    await loader.writeFlash({
      fileArray: files,
      flashMode: "keep",
      flashFreq: "keep",
      flashSize: "keep",
      eraseAll: false,
      compress: true,
      reportProgress: (i, written, size) => {
        const what = files[i].address === 0 ? "firmware" : "speech model";
        step(`Writing ${what}…`, (100 * (before(i) + (written / size) * sizes[i])) / total);
      },
    });
    step("Restarting the board…", 100);
    await loader.after("hard_reset");
    result("Done. The board restarts and is ready in about a second: hold a pad or button (P4: BOOT) and speak.", true);
  } catch (e) {
    if (e.name === "NotFoundError") {
      result("No port selected.", false);
    } else {
      result(`Flashing failed: ${e.message || e}`, false);
      terminal.writeLine(String(e.stack || e));
    }
  } finally {
    try { if (transport) await transport.disconnect(); } catch { /* already closed */ }
    $("flash").disabled = false;
  }
}

if (!("serial" in navigator)) {
  $("unsupported").hidden = false;
  $("flash").disabled = true;
}
$("flash").addEventListener("click", flash);

fetch("version.json")
  .then((r) => (r.ok ? r.json() : null))
  .then((v) => { if (v) $("version").textContent = `Version ${v.version} (${v.date}).`; })
  .catch(() => {});
