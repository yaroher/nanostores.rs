// Boundary benchmark: pure JS nanostores vs the wasm bridge, in one page.
//
// Scenarios (one active listener everywhere, values alternate a/b so every
// write is a real change):
//   set:js          — plain JS nanostores atom.set                       (baseline)
//   set:projection  — projection.set: JS → wasm → notify → JS atom       (full round trip)
//   set:handle      — raw wasm handle.set with a JS callback             (no JS atom on top)
//   set:rust-tick   — mutation inside Rust, only the notification crosses
//   get:js/projection/handle — read cost; projections serve a cached JS value
//
// Counting scenarios (not timed): batched coalescing and unchanged-write skip.
//
// Results are written to the DOM and to window.__benchResults;
// window.__benchDone is set when finished (the CDP driver polls it).

import { atom as jsAtom } from "nanostores";
import { projectStores } from "nanostores-wasm";
import initWasmCore, {
  stores as rawStores,
  tick_large,
  tick_list_order,
  tick_list_row,
  tick_medium,
  tick_scalar,
  tick_small,
  tick_text,
} from "./pkg/bench_app_core";

interface Row {
  id: number;
  name: string;
  score: number;
  active: boolean;
  tag: string;
}

interface RowsPayload {
  rows: Row[];
}

interface WritableLike<T> {
  get(): T;
  set(value: T): void;
  subscribe(callback: () => void): () => void;
}

interface RawHandle<T> {
  get(): T;
  set(value: T): void;
  subscribe(callback: () => void): { unsubscribe(): void };
}

interface ReadableLike<T> {
  get(): T;
  subscribe(callback: () => void): () => void;
}

interface CollectionProjectionLike<V> {
  order: WritableLike<string[]>;
  getRow(key: string): ReadableLike<V | undefined>;
}

interface Payload<T> {
  key: string;
  name: string;
  a: T;
  b: T;
  tick?: () => void;
  benchReads?: boolean;
}

interface RateResult {
  kind: "rate";
  scenario: string;
  payload: string;
  payloadBytes: number;
  opsPerSec: number;
  bestOpsPerSec: number;
}

interface CountResult {
  kind: "count";
  scenario: string;
  payload: string;
  writes: number;
  callbacks: number;
}

type BenchResult = RateResult | CountResult;

const ROUNDS = 5;
const TARGET_MS = 350;

function makeRow(i: number, variant: 0 | 1): Row {
  return {
    id: i + variant * 1_000_000,
    name: `row-${variant}-${String(i).padStart(5, "0")}`,
    score: i * 0.5 + variant,
    active: (i + variant) % 2 === 0,
    tag: variant ? "bench-b" : "bench-a",
  };
}

function makeRows(count: number, variant: 0 | 1): RowsPayload {
  return { rows: Array.from({ length: count }, (_, i) => makeRow(i, variant)) };
}

const TEXT_A = "nanostores-rs bench payload a a a a a a a ";
const TEXT_B = "nanostores-rs bench payload b b b b b b b ";

const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

async function benchSync(
  results: BenchResult[],
  scenario: string,
  payload: Payload<unknown>,
  fn: () => void,
): Promise<void> {
  fn();
  fn();
  fn();

  let n = 256;
  for (;;) {
    const t0 = performance.now();
    for (let i = 0; i < n; i++) fn();
    if (performance.now() - t0 >= 60 || n >= 16_777_216) break;
    n *= 4;
  }

  const reps = Math.max(n, 200);
  const rates: number[] = [];
  for (let round = 0; round < ROUNDS; round++) {
    const t0 = performance.now();
    for (let i = 0; i < reps; i++) fn();
    const elapsed = performance.now() - t0;
    rates.push(reps / (elapsed / 1000));
    await settle();
  }
  rates.sort((x, y) => x - y);

  results.push({
    kind: "rate",
    scenario,
    payload: payload.name,
    payloadBytes: JSON.stringify(payload.a).length,
    opsPerSec: Math.round(rates[Math.floor(ROUNDS / 2)]),
    bestOpsPerSec: Math.round(rates[ROUNDS - 1]),
  });
}

function alternatingSet(
  store: { set(value: unknown): void },
  payload: Payload<unknown>,
): () => void {
  let i = 0;
  return () => store.set(alternating(i++, payload));
}

function alternating(i: number, payload: Payload<unknown>): unknown {
  return i % 2 === 0 ? payload.a : payload.b;
}

async function run(): Promise<BenchResult[]> {
  await initWasmCore();
  const handles = rawStores();
  const projected = projectStores(handles, {
    scalar: "atom",
    text: "atom",
    small: "atom",
    medium: "atom",
    large: "atom",
    batched_scalar: "readable",
    list: "collection",
  }) as unknown as Record<string, WritableLike<unknown>>;

  const listProjection = projected.list as unknown as CollectionProjectionLike<unknown>;

  const payloads: Payload<unknown>[] = [
    { key: "scalar", name: "scalar i32", a: 0, b: 1, tick: tick_scalar },
    { key: "text", name: "text 41B", a: TEXT_A, b: TEXT_B, tick: tick_text },
    { key: "small", name: "row ~120B", a: makeRow(0, 0), b: makeRow(0, 1), tick: tick_small, benchReads: true },
    { key: "medium", name: "120 rows ~10KB", a: makeRows(120, 0), b: makeRows(120, 1), tick: tick_medium },
    { key: "large", name: "1250 rows ~105KB", a: makeRows(1250, 0), b: makeRows(1250, 1), tick: tick_large, benchReads: true },
  ];

  const rawHandles = handles as unknown as Record<string, RawHandle<unknown>>;
  const results: BenchResult[] = [];
  let sink = 0;

  for (const payload of payloads) {
    // 1. Baseline: plain JS nanostores atom, one subscriber.
    {
      const store = jsAtom(payload.a);
      const un = store.subscribe(() => sink++);
      await benchSync(results, "set:js-atom", payload, alternatingSet(store, payload));
      un();
    }

    // 2. Full round trip through the projection (what app code touches).
    {
      const store = projected[payload.key];
      const un = store.subscribe(() => sink++);
      await benchSync(results, "set:projection", payload, alternatingSet(store, payload));
      un();
    }

    // 3. Raw handle: same boundary work, no JS atom on top.
    {
      const handle = rawHandles[payload.key];
      const sub = handle.subscribe(() => sink++);
      await benchSync(results, "set:handle", payload, alternatingSet(handle, payload));
      sub.unsubscribe();
    }

    // 3b. Handle with no subscribers at all: the input crossing
    // (deserialize + store) is paid, the notify crossing is not.
    {
      const handle = rawHandles[payload.key];
      await benchSync(results, "set:handle-nosub", payload, alternatingSet(handle, payload));
    }

    // 4. Rust-side mutation: only the notification crosses the boundary.
    {
      const store = projected[payload.key];
      const un = store.subscribe(() => sink++);
      await benchSync(results, "set:rust-tick", payload, payload.tick!);
      un();
    }

    // 5. Reads, where read cost is meaningful.
    if (payload.benchReads) {
      const store = jsAtom(payload.a);
      await benchSync(results, "get:js-atom", payload, () => {
        sink += Object.is(store.get(), null) ? 1 : 0;
      });

      const projection = projected[payload.key];
      await benchSync(results, "get:projection", payload, () => {
        sink += Object.is(projection.get(), null) ? 1 : 0;
      });

      const handle = rawHandles[payload.key];
      await benchSync(results, "get:handle", payload, () => {
        sink += Object.is(handle.get(), null) ? 1 : 0;
      });
    }
  }

  // Collection: the same 1250 rows, but keyed. Editing one row crosses only
  // that row; re-sorting crosses only the key array.
  {
    const payload: Payload<unknown> = {
      key: "list",
      name: "collection 1250 rows",
      a: makeRows(1250, 0),
      b: makeRows(1250, 1),
    };

    const row = listProjection.getRow("0");
    const unRow = row.subscribe(() => sink++);
    await benchSync(results, "list:row-edit", payload, tick_list_row);
    unRow();

    const unOrder = listProjection.order.subscribe(() => sink++);
    await benchSync(results, "list:reorder", payload, tick_list_order);
    unOrder();
  }

  // Counting scenario: a row edit wakes only that row — neighbours and the
  // order atom stay asleep.
  {
    let row0 = 0;
    let row1 = 0;
    let row2 = 0;
    let orderCallbacks = 0;
    const un0 = listProjection.getRow("0").subscribe(() => row0++);
    const un1 = listProjection.getRow("1").subscribe(() => row1++);
    const un2 = listProjection.getRow("2").subscribe(() => row2++);
    const unOrder = listProjection.order.subscribe(() => orderCallbacks++);
    const initial = row0 + row1 + row2 + orderCallbacks;

    for (let i = 0; i < 100; i++) tick_list_row();
    await settle();
    await settle();

    const woken = row0 + row1 + row2 + orderCallbacks - initial;
    results.push({
      kind: "count",
      scenario: "collection:row-edit",
      payload: "1250 rows",
      writes: 100,
      callbacks: woken,
    });
    un0();
    un1();
    un2();
    unOrder();
  }

  // Counting scenario: batched coalescing.
  {
    let atomCallbacks = 0;
    let batchedCallbacks = 0;
    const unAtom = projected.scalar.subscribe(() => atomCallbacks++);
    const unBatched = projected.batched_scalar.subscribe(() => batchedCallbacks++);
    for (let i = 0; i < 100; i++) projected.scalar.set(i);
    await settle();
    await settle();
    unAtom();
    unBatched();
    results.push({
      kind: "count",
      scenario: "coalescing:atom",
      payload: "scalar i32",
      writes: 100,
      callbacks: atomCallbacks,
    });
    results.push({
      kind: "count",
      scenario: "coalescing:batched",
      payload: "scalar i32",
      writes: 100,
      callbacks: batchedCallbacks,
    });
  }

  // Counting scenario: unchanged writes are skipped on the Rust side.
  {
    let bridgeCallbacks = 0;
    let jsCallbacks = 0;
    const same = makeRow(7, 0);
    const unBridge = projected.small.subscribe(() => bridgeCallbacks++);
    for (let i = 0; i < 1000; i++) projected.small.set(same);
    await settle();
    unBridge();

    const store = jsAtom(same);
    const unJs = store.subscribe(() => jsCallbacks++);
    for (let i = 0; i < 1000; i++) store.set(same);
    unJs();
    results.push({
      kind: "count",
      scenario: "unchanged:projection",
      payload: "row ~120B",
      writes: 1000,
      callbacks: bridgeCallbacks,
    });
    results.push({
      kind: "count",
      scenario: "unchanged:js-atom",
      payload: "row ~120B",
      writes: 1000,
      callbacks: jsCallbacks,
    });
  }

  return results;
}

function renderTable(results: BenchResult[]): string {
  const rates = results.filter((r): r is RateResult => r.kind === "rate");
  const counts = results.filter((r): r is CountResult => r.kind === "count");
  const lines: string[] = [];

  const byPayload = new Map<string, RateResult[]>();
  for (const r of rates) {
    const list = byPayload.get(r.payload) ?? [];
    list.push(r);
    byPayload.set(r.payload, list);
  }

  for (const [payload, rows] of byPayload) {
    lines.push(`### ${payload} (${rows[0].payloadBytes} B)`);
    lines.push("");
    lines.push("| scenario | ops/sec | µs/op |");
    lines.push("| --- | ---: | ---: |");
    for (const r of rows) {
      const us = (1_000_000 / r.opsPerSec).toFixed(2);
      lines.push(`| ${r.scenario} | ${r.opsPerSec.toLocaleString("en-US")} | ${us} |`);
    }
    lines.push("");
  }

  lines.push("### Behavioural counters");
  lines.push("");
  lines.push("| scenario | writes | callbacks |");
  lines.push("| --- | ---: | ---: |");
  for (const r of counts) {
    lines.push(`| ${r.scenario} | ${r.writes} | ${r.callbacks} |`);
  }
  return lines.join("\n");
}

async function main(): Promise<void> {
  const results = await run();
  const env = {
    userAgent: navigator.userAgent,
    hardwareConcurrency: navigator.hardwareConcurrency,
    startedAt: new Date().toISOString(),
  };
  (window as unknown as Record<string, unknown>).__benchResults = { env, results };
  document.getElementById("bench-table")!.textContent = renderTable(results);
  document.getElementById("bench-json")!.textContent = JSON.stringify({ env, results }, null, 2);
  document.getElementById("status")!.textContent = "done";
  (window as unknown as Record<string, unknown>).__benchDone = true;
}

main().catch((error: unknown) => {
  document.getElementById("status")!.textContent = `failed: ${String(error)}`;
  document.getElementById("bench-json")!.textContent = String(
    (error as { stack?: string })?.stack ?? error,
  );
  (window as unknown as Record<string, unknown>).__benchDone = true;
});
