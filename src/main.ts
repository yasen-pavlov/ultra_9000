// The global visual system loads first so component styles win specificity ties.
import "./app.css";
import { mount } from "svelte";
import App from "./App.svelte";

const target = document.getElementById("app");
if (!target) throw new Error("Missing #app mount element");

export default mount(App, { target });
