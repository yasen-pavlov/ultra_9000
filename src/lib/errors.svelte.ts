// Failures of commands the web view issued (a rejected launch, a failed settings write). Native
// launch failures and ended processes arrive as native notices; these are only local, dismissible.
import { errorMessage } from "./native";

export interface UiError {
  id: number;
  message: string;
}

const MAX_ERRORS = 3;

class UiErrors {
  list = $state<UiError[]>([]);
  #next = 1;

  report(message: string): void {
    this.list = [...this.list.filter((entry) => entry.message !== message), { id: this.#next++, message }].slice(
      -MAX_ERRORS,
    );
  }

  dismiss(id: number): void {
    this.list = this.list.filter((entry) => entry.id !== id);
  }
}

export const uiErrors = new UiErrors();

/** Runs a native command; a rejection becomes a dismissible "<label> failed: …" message. */
export async function run<T>(label: string, action: () => Promise<T>): Promise<T | undefined> {
  try {
    return await action();
  } catch (error) {
    uiErrors.report(`${label} failed: ${errorMessage(error)}`);
    return undefined;
  }
}
