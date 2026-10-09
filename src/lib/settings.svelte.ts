// Mirrors the native SettingsState (files are the source of truth; native loads, validates and
// writes them). The UI reads effective values with built-in fallbacks while the first state loads.
import {
  errorMessage,
  fetchSettingsState,
  listenSettingsState,
  type SettingScalar,
  type SettingsState,
  type SettingValue,
} from "./native";

export type Density = "comfortable" | "compact";
export type ProviderMarkMode = "corner" | "off";
export type BrandColour = "all" | "pickers" | "launch" | "off";
export type ContextBarMode = "auto" | "always" | "never";

/** Key of a project's default tool override, exactly as the contract spells it. */
export function projectToolKey(projectPath: string): string {
  return `projects.${JSON.stringify(projectPath)}.default_tool`;
}

function pick<T extends string>(value: SettingScalar | undefined, allowed: readonly T[], fallback: T): T {
  return allowed.find((candidate) => candidate === value) ?? fallback;
}

function finite(value: SettingScalar | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

class SettingsStore {
  state = $state.raw<SettingsState | null>(null);
  error = $state<string | null>(null);

  values: ReadonlyMap<string, SettingValue> = $derived(
    new Map((this.state?.values ?? []).map((entry) => [entry.key, entry])),
  );

  /** Built-in defaults match the native settings defaults until native state arrives. */
  sidebarWidth: number = $derived(Math.min(360, Math.max(240, finite(this.values.get("sidebar.width")?.value, 304))));
  density: Density = $derived(
    pick(this.values.get("sidebar.density")?.value, ["comfortable", "compact"], "comfortable"),
  );
  providerMark: ProviderMarkMode = $derived(
    pick(this.values.get("sidebar.provider_mark")?.value, ["corner", "off"], "corner"),
  );
  brandColour: BrandColour = $derived(
    pick(this.values.get("icons.brand_color")?.value, ["all", "pickers", "launch", "off"], "all"),
  );
  autoCollapseBelow: number = $derived(finite(this.values.get("sidebar.auto_collapse_below")?.value, 800));
  contextBar: ContextBarMode = $derived(
    pick(this.values.get("layout.context_bar")?.value, ["auto", "always", "never"], "auto"),
  );
  hasErrors: boolean = $derived((this.state?.errors.length ?? 0) > 0);

  #disconnect: (() => void) | null = null;

  #apply(next: SettingsState): void {
    if (this.state !== null && next.revision <= this.state.revision) return;
    this.state = next;
    this.error = null;
  }

  /** Subscribe first, then fetch. Returns cleanup. */
  connect(): () => void {
    this.#disconnect?.();
    let disposed = false;
    let unlisten: (() => void) | null = null;
    const disconnect = () => {
      disposed = true;
      unlisten?.();
      unlisten = null;
      if (this.#disconnect === disconnect) this.#disconnect = null;
    };
    this.#disconnect = disconnect;

    void (async () => {
      try {
        const stop = await listenSettingsState((next) => {
          if (!disposed) this.#apply(next);
        });
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
      } catch (error) {
        if (!disposed) this.error = `Cannot subscribe to settings: ${errorMessage(error)}`;
        return;
      }
      try {
        const initial = await fetchSettingsState();
        if (!disposed) this.#apply(initial);
      } catch (error) {
        if (!disposed && this.state === null) this.error = `Cannot read settings: ${errorMessage(error)}`;
      }
    })();

    return () => this.#disconnect?.();
  }

  retry(): void {
    this.error = null;
    this.connect();
  }
}

export const settings = new SettingsStore();
