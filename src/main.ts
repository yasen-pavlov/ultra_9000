// The global visual system loads first so component styles win specificity ties.
import "./app.css";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { mount } from "svelte";
import App from "./App.svelte";
import Popover from "./popover/Popover.svelte";

const target = document.getElementById("app");
if (!target) throw new Error("Missing #app mount element");

/** The macOS menu-bar popover is a second window on the same bundle (label `popover`). */
function isPopover(): boolean {
  try {
    return getCurrentWindow().label === "popover";
  } catch {
    // Outside Tauri (a plain browser preview) there is no window metadata: the main window.
    return false;
  }
}

const popover = isPopover();
if (popover) document.documentElement.classList.add("popover");

export default popover ? mount(Popover, { target }) : mount(App, { target });
