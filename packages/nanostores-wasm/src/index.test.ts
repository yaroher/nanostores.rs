import { describe, expect, it, vi } from "vitest";
import { cleanStores, listenKeys } from "nanostores";
import {
  projectAtom,
  projectMap,
  projectReadable,
  projectStores,
  type AtomHandle,
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
