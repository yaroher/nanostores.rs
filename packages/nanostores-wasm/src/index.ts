import {
  atom,
  map,
  onMount,
  type MapStore,
  type ReadableAtom,
  type WritableAtom,
} from "nanostores";

type AllKeys<T> = T extends any ? keyof T : never;
type StringKey<T> = Extract<AllKeys<T>, string>;
export type StoreKind = "atom" | "map" | "readable" | "collection";
export type StoreKinds<Handles> = { [Key in keyof Handles]: StoreKind };

export interface SubscriptionHandle {
  unsubscribe?(): void;
  free?(): void;
}

export interface AtomHandle<T = unknown> {
  get(): T;
  set(value: T): void;
  subscribe(callback: (value: T) => void): SubscriptionHandle;
}

export interface ReadableHandle<T = unknown> {
  get(): T;
  subscribe(callback: (value: T) => void): SubscriptionHandle;
}

export interface MapHandle<T extends object = Record<string, unknown>> {
  get(): T;
  set(value: T): void;
  setKey<K extends StringKey<T>>(key: K, value: T[K]): void;
  subscribe(callback: (value: T, changedKey?: StringKey<T>) => void): SubscriptionHandle;
}

export interface CollectionHandle<V = unknown> {
  get(): V[];
  getItem(key: string): V | undefined;
  order(): string[];
  subscribe(callback: (rows: V[]) => void): SubscriptionHandle;
  subscribeKey(key: string, callback: (row: V | undefined) => void): SubscriptionHandle;
  subscribeOrder(callback: (keys: string[]) => void): SubscriptionHandle;
}

export interface CollectionProjection<V> {
  /** Keys in render order — a real nanostores atom, so `useStore` works. */
  order: WritableAtom<string[]>;
  /**
   * A real nanostores atom for one row. Lazy: the wasm subscription is held
   * only while the atom has listeners. Rows are read-only from JS — mutations
   * belong to the Rust side. A removed row notifies `undefined`.
   *
   * Atoms are cached per key for the lifetime of the projection; editing one
   * row wakes only that row's atom, and a re-sort wakes only `order`.
   */
  getRow(key: string): ReadableAtom<V | undefined>;
}

export type ProjectedStore<Handle, Kind extends StoreKind> = Kind extends "map"
  ? Handle extends MapHandle<infer Value>
    ? MapStore<Value>
    : never
  : Kind extends "readable"
    ? Handle extends ReadableHandle<infer Value>
      ? ReadableAtom<Value>
      : never
    : Kind extends "collection"
      ? Handle extends CollectionHandle<infer Value>
        ? CollectionProjection<Value>
        : never
      : Handle extends AtomHandle<infer Value>
        ? WritableAtom<Value>
        : never;

export type ProjectedStores<
  Handles,
  Kinds extends StoreKinds<Handles>,
> = {
  [Key in keyof Handles]: ProjectedStore<Handles[Key], Kinds[Key]>;
};

export function projectAtom<T>(handle: AtomHandle<T>): WritableAtom<T> {
  const projection = atom<T>(handle.get());
  const apply = projection.set.bind(projection);
  let mounted = false;
  let subscription: SubscriptionHandle | undefined;

  onMount(projection, () => {
    mounted = true;
    subscription = handle.subscribe((value) => apply(value));
    apply(handle.get());

    return () => {
      mounted = false;
      releaseSubscription(subscription);
      subscription = undefined;
    };
  });

  projection.set = (value: T) => {
    try {
      handle.set(value);
      if (!mounted) apply(handle.get());
    } catch (error) {
      console.error(error);
    }
  };

  return projection;
}

export function projectReadable<T>(handle: ReadableHandle<T>): ReadableAtom<T> {
  const projection = atom<T>(handle.get());
  const apply = projection.set.bind(projection);
  let subscription: SubscriptionHandle | undefined;

  onMount(projection, () => {
    subscription = handle.subscribe((value) => apply(value));
    apply(handle.get());

    return () => {
      releaseSubscription(subscription);
      subscription = undefined;
    };
  });

  return projection;
}

export function projectMap<T extends object>(handle: MapHandle<T>): MapStore<T> {
  const projection = map<T>(handle.get());
  const applySet = projection.set.bind(projection);
  const applySetKey = projection.setKey.bind(projection);
  let mounted = false;
  let subscription: SubscriptionHandle | undefined;

  onMount(projection, () => {
    mounted = true;
    subscription = handle.subscribe((value, changedKey) => {
      if (changedKey === undefined) {
        applySet(value);
        return;
      }

      applyChangedKey(applySetKey, value, changedKey);
    });
    applySet(handle.get());

    return () => {
      mounted = false;
      releaseSubscription(subscription);
      subscription = undefined;
    };
  });

  projection.set = (value: T) => {
    try {
      handle.set(value);
      if (!mounted) applySet(handle.get());
    } catch (error) {
      console.error(error);
    }
  };

  projection.setKey = (<K extends StringKey<T>>(key: K, value: T[K]) => {
    try {
      handle.setKey(key, value);
      if (!mounted) applySet(handle.get());
    } catch (error) {
      console.error(error);
    }
  }) as MapStore<T>["setKey"];

  return projection;
}

export function projectStores<
  Handles extends object,
  Kinds extends StoreKinds<Handles>,
>(handles: Handles, kinds: Kinds): ProjectedStores<Handles, Kinds> {
  const projected: Partial<Record<keyof Handles, unknown>> = {};

  for (const key of Object.keys(kinds as object) as Array<keyof Handles & string>) {
    const kind = kinds[key];
    const handle = handles[key];

    switch (kind) {
      case "atom":
        projected[key] = projectAtom(handle as AtomHandle<unknown>);
        break;
      case "map":
        projected[key] = projectMap(handle as MapHandle<Record<string, unknown>>);
        break;
      case "readable":
        projected[key] = projectReadable(handle as ReadableHandle<unknown>);
        break;
      case "collection":
        projected[key] = projectCollection(handle as CollectionHandle<unknown>);
        break;
    }
  }

  return projected as ProjectedStores<Handles, Kinds>;
}

export function projectCollection<V>(handle: CollectionHandle<V>): CollectionProjection<V> {
  const order = atom<string[]>(handle.order());
  const applyOrder = order.set.bind(order);
  onMount(order, () => {
    const subscription = handle.subscribeOrder((keys) => applyOrder(keys));
    applyOrder(handle.order());

    return () => {
      releaseSubscription(subscription);
    };
  });

  const rows = new Map<string, ReadableAtom<V | undefined>>();

  function getRow(key: string): ReadableAtom<V | undefined> {
    let row = rows.get(key);
    if (row) return row;

    const rowAtom = atom<V | undefined>(handle.getItem(key));
    const apply = rowAtom.set.bind(rowAtom);
    onMount(rowAtom, () => {
      const subscription = handle.subscribeKey(key, (value) => apply(value));
      apply(handle.getItem(key));

      return () => {
        releaseSubscription(subscription);
      };
    });
    rows.set(key, rowAtom);
    return rowAtom;
  }

  return { order, getRow };
}

function releaseSubscription(subscription: SubscriptionHandle | undefined): void {
  subscription?.unsubscribe?.();
  subscription?.free?.();
}

function applyChangedKey<T extends object>(
  setKey: MapStore<T>["setKey"],
  value: T,
  key: StringKey<T>,
): void {
  setKey(key, value[key as keyof T] as never);
}
