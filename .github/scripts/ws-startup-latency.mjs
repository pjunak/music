import { performance } from "node:perf_hooks";

const [url, rawIterations = "40", rawWarmups = "5"] = process.argv.slice(2);
const iterations = Number.parseInt(rawIterations, 10);
const warmups = Number.parseInt(rawWarmups, 10);

if (!url || !Number.isInteger(iterations) || iterations < 1 || iterations > 10_000) {
  throw new Error("usage: node ws-startup-latency.mjs <ws-url> <iterations> [warmups]");
}
if (!Number.isInteger(warmups) || warmups < 0 || warmups > 1_000) {
  throw new Error("warmups must be an integer from 0 through 1000");
}
if (typeof WebSocket !== "function") {
  throw new Error("this benchmark requires Node.js with the global WebSocket API");
}

async function connectionToSnapshot(sample) {
  return new Promise((resolve, reject) => {
    const startedAt = performance.now();
    const socket = new WebSocket(url);
    let settled = false;
    let measuredElapsedMs;
    const timeout = setTimeout(() => {
      fail(new Error(`WebSocket sample ${sample} timed out`));
    }, 5_000);

    function fail(error) {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      if (socket.readyState === WebSocket.OPEN) socket.close(1000);
      reject(error);
    }

    function succeed() {
      if (settled || measuredElapsedMs === undefined) return;
      settled = true;
      clearTimeout(timeout);
      resolve(measuredElapsedMs);
    }

    socket.addEventListener("open", () => {
      socket.send(
        JSON.stringify({
          type: "register",
          name: "Runtime benchmark",
          client_id: `runtime-benchmark-${sample}`,
          protocol_version: 2,
        }),
      );
    });
    socket.addEventListener("message", (event) => {
      let message;
      try {
        message = JSON.parse(String(event.data));
      } catch {
        return;
      }
      if (message?.type === "state_snapshot" || message?.type === "state_changed") {
        if (measuredElapsedMs === undefined) {
          measuredElapsedMs = performance.now() - startedAt;
          socket.close(1000);
        }
      }
    });
    socket.addEventListener("error", () => {
      if (measuredElapsedMs === undefined) {
        fail(new Error(`WebSocket sample ${sample} failed`));
      } else {
        succeed();
      }
    });
    socket.addEventListener("close", () => {
      if (settled) return;
      if (measuredElapsedMs === undefined) {
        fail(new Error(`WebSocket sample ${sample} closed before a state frame`));
        return;
      }
      succeed();
    });
  });
}

for (let sample = 0; sample < warmups; sample += 1) {
  await connectionToSnapshot(`warmup-${sample}`);
}
for (let sample = 0; sample < iterations; sample += 1) {
  const elapsedMs = await connectionToSnapshot(sample);
  process.stdout.write(`${elapsedMs.toFixed(3)}\n`);
}
