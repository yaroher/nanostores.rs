import { describe, expect, it, vi } from "vitest";
import { cleanStores, listenKeys } from "nanostores";
import {
  projectAtom,
  projectCollection,
  projectMap,
  projectReadable,
  projectStores,
  type AtomHandle,
  type CollectionHandle,
  type MapHandle,
  type ReadableHandle,
  type SubscriptionHandle,
} from "./index";

class FakeSubscription implements SubscriptionHandle {
  #cleanup: (() => void) | undefined;
  unsubscribed = false;
  freed = false;

  constructor(cleanup: () => void) {
    this.#cleanup = cleanup;
  }

  unsubscribe(): void {
    this.unsubscribed = true;
    this.#cleanup?.();
    this.#cleanup = undefined;
  }

  free(): void {
    this.freed = true;
    this.#cleanup?.();
    this.#cleanup = undefined;
  }
}

class FakeAtomHandle<T> implements AtomHandle<T>, ReadableHandle<T> {
  value: T;
  callbacks = new Set<(value: T) => void>();
  subscribeCalls = 0;
  lastSubscription: FakeSubscription | undefined;

  constructor(value: T) {
    this.value = value;
  }

  get(): T {
    return this.value;
  }

  set(value: T): void {
    this.value = value;
    for (const callback of this.callbacks) callback(value);
  }

  subscribe(callback: (value: T) => void): SubscriptionHandle {
    this.subscribeCalls += 1;
    this.callbacks.add(callback);
    this.lastSubscription = new FakeSubscription(() => this.callbacks.delete(callback));
    return this.lastSubscription;
  }
}

class FakeMapHandle<T extends object> implements MapHandle<T> {
  value: T;
  callbacks = new Set<Parameters<MapHandle<T>["subscribe"]>[0]>();
  subscribeCalls = 0;

  constructor(value: T) {
    this.value = value;
  }

  get(): T {
    return this.value;
  }

  set(value: T): void {
    this.value = value;
    for (const callback of this.callbacks) callback(value);
  }

  setKey: MapHandle<T>["setKey"] = (key, value) => {
    this.value = { ...this.value, [key]: value };
    for (const callback of this.callbacks) callback(this.value, key);
  };

  subscribe: MapHandle<T>["subscribe"] = (callback) => {
    this.subscribeCalls += 1;
    this.callbacks.add(callback);
    return new FakeSubscription(() => this.callbacks.delete(callback));
  };
}

describe("projectAtom", () => {
  it("mounts lazily and applies writes from the wasm callback", () => {
    const handle = new FakeAtomHandle(1);
    const projection = projectAtom(handle);

    expect(handle.subscribeCalls).toBe(0);
    expect(projection.get()).toBe(1);

    const seen: number[] = [];
    const unbind = projection.subscribe((value) => seen.push(value));
    projection.set(2);

    expect(handle.value).toBe(2);
    expect(seen).toEqual([1, 2]);
    expect(handle.subscribeCalls).toBe(1);

    unbind();
  });

  it("keeps local value synced for unmounted writes", () => {
    const handle = new FakeAtomHandle(1);
    const projection = projectAtom(handle);

    projection.set(5);

    expect(projection.get()).toBe(5);
  });

  it("logs rejected writes without locally applying them", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const handle = new FakeAtomHandle(1);
    handle.set = () => {
      throw new Error("bad value");
    };
    const projection = projectAtom(handle);

    projection.set(2);

    expect(projection.get()).toBe(1);
    expect(consoleError).toHaveBeenCalledOnce();
    consoleError.mockRestore();
  });

  it("releases the wasm subscription after unmount", () => {
    const handle = new FakeAtomHandle(1);
    const projection = projectAtom(handle);

    const unbind = projection.subscribe(() => {});
    unbind();
    cleanStores(projection);

    expect(handle.lastSubscription?.unsubscribed).toBe(true);
    expect(handle.callbacks.size).toBe(0);
  });
});

describe("projectReadable", () => {
  it("projects a read-only handle as a nanostores readable atom", () => {
    const handle = new FakeAtomHandle(2);
    const projection = projectReadable(handle);
    const seen: number[] = [];

    const unbind = projection.subscribe((value) => seen.push(value));
    handle.set(3);

    expect(seen).toEqual([2, 3]);
    unbind();
  });

  it("frees subscriptions when handles expose only free", () => {
    const handle = new FakeAtomHandle(2);
    const projection = projectReadable({
      get: () => handle.get(),
      subscribe: (callback) => {
        handle.subscribe(callback);
        return { free: () => handle.lastSubscription?.free() };
      },
    });

    const unbind = projection.subscribe(() => {});
    unbind();
    cleanStores(projection);

    expect(handle.lastSubscription?.freed).toBe(true);
  });
});

describe("projectMap", () => {
  it("uses real map setKey updates so listenKeys stays native", () => {
    const handle = new FakeMapHandle({ name: "Ada", age: 36 });
    const projection = projectMap(handle);
    const names: string[] = [];
    const unbind = listenKeys(projection, ["name"], (value) => names.push(value.name));

    projection.setKey("age", 37);
    projection.setKey("name", "Grace");

    expect(handle.value).toEqual({ name: "Grace", age: 37 });
    expect(names).toEqual(["Grace"]);
    unbind();
  });

  it("does not apply rejected setKey locally", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const handle = new FakeMapHandle({ name: "Ada", age: 36 });
    handle.setKey = () => {
      throw new Error("rejected");
    };
    const projection = projectMap(handle);

    projection.setKey("name", "Grace");

    expect(projection.get()).toEqual({ name: "Ada", age: 36 });
    expect(consoleError).toHaveBeenCalledOnce();
    consoleError.mockRestore();
  });
});

describe("projectStores", () => {
  it("projects a generated handle object from generated store kinds", () => {
    const handles = {
      count: new FakeAtomHandle(1),
      user: new FakeMapHandle({ name: "Ada" }),
      doubled: new FakeAtomHandle(2),
    };
    const projected = projectStores(handles, {
      count: "atom",
      user: "map",
      doubled: "readable",
    });

    expect(projected.count.get()).toBe(1);
    expect(projected.user.get()).toEqual({ name: "Ada" });
    expect(projected.doubled.get()).toBe(2);

    projected.count.set(3);
    projected.user.setKey("name", "Grace");

    expect(handles.count.value).toBe(3);
    expect(handles.user.value).toEqual({ name: "Grace" });
  });
});

class FakeCollectionHandle<V> implements CollectionHandle<V> {
  rows: Map<string, V>;
  orderKeys: string[];
  rowCallbacks = new Map<string, Set<(row: V | undefined) => void>>();
  orderCallbacks = new Set<(keys: string[]) => void>();

  constructor(rows: Record<string, V>, orderKeys?: string[]) {
    this.rows = new Map(Object.entries(rows));
    this.orderKeys = orderKeys ?? Object.keys(rows);
  }

  get(): V[] {
    return this.orderKeys
      .map((key) => this.rows.get(key))
      .filter((row): row is V => row !== undefined);
  }

  getItem(key: string): V | undefined {
    return this.rows.get(key);
  }

  order(): string[] {
    return [...this.orderKeys];
  }

  subscribe(_callback: (rows: V[]) => void): SubscriptionHandle {
    return new FakeSubscription(() => {});
  }

  subscribeKey(key: string, callback: (row: V | undefined) => void): SubscriptionHandle {
    const callbacks = this.rowCallbacks.get(key) ?? new Set();
    callbacks.add(callback);
    this.rowCallbacks.set(key, callbacks);
    return new FakeSubscription(() => callbacks.delete(callback));
  }

  subscribeOrder(callback: (keys: string[]) => void): SubscriptionHandle {
    this.orderCallbacks.add(callback);
    return new FakeSubscription(() => this.orderCallbacks.delete(callback));
  }

  // Test-side mutations mimicking the Rust store's behaviour.
  setRow(key: string, value: V): void {
    const existed = this.rows.has(key);
    this.rows.set(key, value);
    if (!existed) this.orderKeys.push(key);
    for (const callback of this.rowCallbacks.get(key) ?? []) callback(value);
  }

  removeRow(key: string): void {
    this.rows.delete(key);
    this.orderKeys = this.orderKeys.filter((k) => k !== key);
    for (const callback of this.rowCallbacks.get(key) ?? []) callback(undefined);
    for (const callback of this.orderCallbacks) callback([...this.orderKeys]);
  }

  reorder(keys: string[]): void {
    this.orderKeys = [...keys];
    for (const callback of this.orderCallbacks) callback([...this.orderKeys]);
  }
}

describe("projectCollection", () => {
  it("order is a real atom that follows re-sorts", () => {
    const handle = new FakeCollectionHandle({ a: 1, b: 2 });
    const projection = projectCollection(handle);

    const seen: (readonly string[])[] = [];
    const unbind = projection.order.subscribe((keys) => seen.push(keys));

    handle.reorder(["b", "a"]);

    expect(seen[0]).toEqual(["a", "b"]);
    expect(seen[1]).toEqual(["b", "a"]);
    expect(projection.order.get()).toEqual(["b", "a"]);

    unbind();
  });

  it("getRow wakes only its own row, lazily", () => {
    const handle = new FakeCollectionHandle({ a: 1, b: 2 });
    const projection = projectCollection(handle);

    expect(handle.rowCallbacks.size).toBe(0);

    const seenA: (number | undefined)[] = [];
    const unbindA = projection.getRow("a").subscribe((value) => seenA.push(value));

    expect(handle.rowCallbacks.has("a")).toBe(true);
    expect(handle.rowCallbacks.has("b")).toBe(false);

    handle.setRow("b", 22);
    expect(seenA).toEqual([1]);

    handle.setRow("a", 11);
    expect(seenA).toEqual([1, 11]);

    // Same key returns the same atom; it is not re-created.
    expect(projection.getRow("a").get()).toBe(11);

    unbindA();
  });

  it("a removed row notifies undefined and the order atom", () => {
    const handle = new FakeCollectionHandle({ a: 1, b: 2 });
    const projection = projectCollection(handle);

    const seen: (number | undefined)[] = [];
    const unbind = projection.getRow("a").subscribe((value) => seen.push(value));
    const orderSeen: (readonly string[])[] = [];
    const unbindOrder = projection.order.subscribe((keys) => orderSeen.push(keys));

    handle.removeRow("a");

    expect(seen).toEqual([1, undefined]);
    expect(orderSeen[orderSeen.length - 1]).toEqual(["b"]);
    expect(projection.getRow("a").get()).toBeUndefined();

    unbind();
    unbindOrder();
  });

  it("releases row subscriptions after unmount", () => {
    const handle = new FakeCollectionHandle({ a: 1 });
    const projection = projectCollection(handle);

    const row = projection.getRow("a");
    const unbind = row.subscribe(() => {});
    expect(handle.rowCallbacks.get("a")?.size).toBe(1);

    unbind();
    cleanStores(row);

    expect(handle.rowCallbacks.get("a")?.size ?? 0).toBe(0);
  });
});
