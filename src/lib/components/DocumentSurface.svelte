<script lang="ts">
  // The document surface (boards 31–34): a Markdown file rendered in the panel the terminal uses,
  // same background and corners, no toolbar. Native sends text when `document.revision` bumps; this
  // renders it (lazily loading the renderer, Shiki, KaTeX and Mermaid), keeps the reader's place
  // across re-renders (nearest heading + offset), can follow the tail, and says what happened in
  // one line at the bottom right. Reading keys are plain DOM keys: the web view owns the keyboard
  // here, and app chords still reach native first.
  //
  // Trust boundary: the prose is sanitised HTML from render.ts, written into the article once per
  // applied render; the reader's own controls are created here and recognised by node identity
  // (`controls`), never by classes or attributes the document could also produce.
  import { tick } from "svelte";
  import { run } from "../errors.svelte";
  import { formatBytes } from "../format";
  import { clearMatches, firstMatchFrom, markMatches } from "../document/find";
  import { followAfterAction, followAfterKeyScroll, followAfterScroll } from "../document/follow";
  import { frontmatterSummary } from "../document/frontmatter";
  import { loadedHighlighter, loadHighlighter } from "../document/highlight";
  import { documentKey, outlineMode, type DocumentAction } from "../document/keys";
  import { linkTarget } from "../document/links";
  import { renderMermaid } from "../document/mermaid";
  import { capturePlace, restorePlace, type HeadingTop } from "../document/place";
  import type { DocumentHeading, RenderedDocument } from "../document/render";
  import { documentSessions } from "../document/session.svelte";
  import { statusLine } from "../document/status";
  import { acceptsText, wantsText } from "../document/surface-state";
  import {
    documentAsset,
    documentOpenLink,
    documentReload,
    documentText,
    errorMessage,
    itemClose,
    newNeovimForFile,
    openDocument,
    revealDocument,
    type DocumentState,
  } from "../native";
  import DocumentFind from "./DocumentFind.svelte";
  import DocumentMenu, { type DocumentMenuAction } from "./DocumentMenu.svelte";
  import DocumentOutline from "./DocumentOutline.svelte";

  let {
    itemId,
    title,
    doc,
    presentationEpoch,
  }: {
    itemId: string;
    title: string;
    doc: DocumentState;
    /** Native bumps it on every intentional presentation (selection, sheet closed); focus follows it. */
    presentationEpoch: number;
  } = $props();

  type DiagramResult = { svg: string } | { error: string };

  // Per item: the parent keys this component by item id, so these are fixed for its lifetime.
  // svelte-ignore state_referenced_locally
  const memory = documentSessions.memory(itemId);
  const mac = /Mac/.test(navigator.userAgent);
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  /** The reader's own controls in the prose, by identity, with what activating each does. */
  const controls = new WeakMap<Element, (() => void) | null>();
  /** Mermaid results for the diagrams of the current render only, by source. */
  const diagrams = new Map<string, Promise<DiagramResult>>();

  let rendered = $state.raw<RenderedDocument | null>(null);
  let loadError = $state<string | null>(null);
  // svelte-ignore state_referenced_locally
  let follow = $state(memory.follow ?? doc.follow);
  let linesBelow = $state(memory.linesBelow);
  let replaced = $state<{ at: number; kept: boolean } | null>(null);
  let copiedAt = $state<number | null>(null);
  let now = $state(Date.now());
  let panelWidth = $state(0);
  let currentId = $state<string | null>(null);
  let fmOpen = $state(false);
  let menuOpen = $state(false);
  let outlineOpen = $state(false);
  let findOpen = $state(memory.findText !== "");
  let findText = $state(memory.findText);
  let matches = $state.raw<HTMLElement[]>([]);
  let matchIndex = $state(0);
  let footnote = $state<{ html: string; top: number; left: number } | null>(null);

  let root = $state<HTMLElement | null>(null);
  let scroller = $state<HTMLElement | null>(null);
  let content = $state<HTMLElement | null>(null);
  let rail = $state<ReturnType<typeof DocumentOutline> | null>(null);
  let finder = $state<ReturnType<typeof DocumentFind> | null>(null);

  /** Highest revision asked for or received; the snapshot may lag behind a reply. */
  let fetchedRevision = memory.revision;
  /** Revision of the newest text accepted for rendering. */
  let latestRevision = -1;
  let destroyed = false;
  let pendingG = false;
  let programmatic = false;
  let lastTop = 0;
  let observer: IntersectionObserver | null = null;

  const outlineEntries = $derived(
    (rendered?.headings ?? []).filter((heading) => heading.level === 2 || heading.level === 3),
  );
  const mode = $derived(outlineMode(panelWidth, outlineEntries.length, documentSessions.railHidden));
  const relativePath = $derived(doc.dir === "." ? title : `${doc.dir}/${title}`);
  const line = $derived(
    statusLine({
      now,
      lastEvent: doc.lastEvent,
      lastEventAtMs: doc.lastEventAtMs,
      follow,
      linesBelow,
      replaced,
      copiedAt,
    }),
  );
  /** The line shown last, kept while it fades out. */
  let shownLine = $state<{ text: string; clickable: boolean } | null>(null);
  $effect(() => {
    if (line !== null) shownLine = { text: line.text, clickable: line.clickable };
  });

  // Whole-second ticks for the bottom-right line.
  $effect(() => {
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });

  // Memory survives switching away and back.
  $effect(() => {
    memory.follow = follow;
    memory.linesBelow = linesBelow;
    memory.findText = findText;
  });

  // The last text read shows at once on return (also under a missing/unreadable notice); then
  // native is asked whenever it read something newer (surface-state.ts).
  $effect(() => {
    const text = memory.text;
    if (text !== null) void accept(memory.revision, text);
    return () => {
      destroyed = true;
      observer?.disconnect();
      diagrams.clear();
      if (scroller !== null && rendered !== null) memory.place = capturePlace(headingTops(), scroller.scrollTop);
    };
  });

  $effect(() => {
    if (!wantsText(doc, fetchedRevision, true)) return;
    void refresh(doc.revision);
  });

  // Selecting the document (also the one already selected) or closing a sheet over it gives it the
  // keyboard; content revisions never move focus.
  $effect(() => {
    void presentationEpoch;
    scroller?.focus({ preventScroll: true });
  });

  // Matches follow the query (and every re-render, which replaces the marked DOM). A changed query
  // brings its first match into view; a re-render only re-marks.
  let markedQuery = "";
  $effect(() => {
    void rendered;
    const query = findOpen ? findText : "";
    void tick().then(() => {
      mark(query, query !== "" && query !== markedQuery);
      markedQuery = query;
    });
  });

  async function loadAsset(relPath: string): Promise<string | null> {
    let pending = memory.assets.get(relPath);
    if (pending === undefined) {
      pending = documentAsset(itemId, relPath).then(
        (asset) => (asset.mime.startsWith("image/") ? `data:${asset.mime};base64,${asset.base64}` : null),
        () => null,
      );
      memory.assets.set(relPath, pending);
    }
    return pending;
  }

  async function refresh(revision: number): Promise<void> {
    fetchedRevision = Math.max(fetchedRevision, revision);
    try {
      const text = await documentText(itemId);
      if (destroyed) return;
      // A reply may be newer than the snapshot that asked (a reload just finished): keep it.
      fetchedRevision = Math.max(fetchedRevision, text.revision);
      loadError = null;
      await accept(text.revision, text.text);
    } catch (error) {
      if (!destroyed) loadError = errorMessage(error);
    }
  }

  /** Renders a reply unless a newer one was already accepted. */
  async function accept(revision: number, text: string): Promise<void> {
    if (!acceptsText(revision, latestRevision)) return;
    latestRevision = revision;
    memory.text = text;
    memory.revision = revision;
    await draw(revision, text, "revision");
  }

  async function draw(revision: number, text: string, reason: "revision" | "restyle"): Promise<void> {
    const { renderMarkdown } = await import("../document/render");
    const result = await renderMarkdown(text, { loadAsset, highlighter: loadedHighlighter() });
    if (destroyed || revision !== latestRevision) return;
    await apply(result, reason);
    if (result.needsHighlighter) {
      loadHighlighter().then(
        () => {
          if (!destroyed && revision === latestRevision) void draw(revision, text, "restyle");
        },
        () => {
          // Code stays plain mono text; nothing else depends on highlighting.
        },
      );
    }
  }

  function headingTops(): HeadingTop[] {
    if (scroller === null || content === null) return [];
    const base = scroller.getBoundingClientRect().top - scroller.scrollTop;
    return [...content.querySelectorAll<HTMLElement>("h1[id], h2[id], h3[id], h4[id], h5[id], h6[id]")].map(
      (heading) => ({ id: heading.id, top: heading.getBoundingClientRect().top - base }),
    );
  }

  function atEnd(): boolean {
    return scroller === null || scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2;
  }

  function setScroll(top: number, smooth = false): void {
    if (scroller === null) return;
    programmatic = true;
    scroller.scrollTo({ top, behavior: smooth && !reducedMotion.matches ? "smooth" : "auto" });
    lastTop = scroller.scrollTop;
    requestAnimationFrame(() => (programmatic = false));
  }

  function toEnd(): void {
    if (scroller !== null) setScroll(scroller.scrollHeight);
    linesBelow = 0;
  }

  async function apply(result: RenderedDocument, reason: "revision" | "restyle"): Promise<void> {
    if (content === null) return;
    const before =
      rendered === null || scroller === null
        ? null
        : { place: capturePlace(headingTops(), scroller.scrollTop), lineCount: rendered.lineCount };
    // Fresh prose DOM for every applied render, so controls and diagrams are set up exactly once
    // per node even when the HTML did not change (an edit to frontmatter only, a reload). The
    // article has no template children, so Svelte never reconciles inside it.
    // eslint-disable-next-line svelte/no-dom-manipulating -- sanitised HTML from render.ts into an otherwise empty element
    content.innerHTML = result.html;
    decorate(content);
    rendered = result;
    documentSessions.setH1(itemId, result.h1);
    await tick();
    if (follow) {
      toEnd();
      return;
    }
    if (before === null) {
      if (memory.place !== null) setScroll(restorePlace(memory.place, headingTops()).scrollTop);
      return;
    }
    const restored = restorePlace(before.place, headingTops());
    setScroll(restored.scrollTop);
    if (reason === "restyle") return;
    if (doc.lastEvent === "replaced") {
      replaced = { at: doc.lastEventAtMs > 0 ? doc.lastEventAtMs : Date.now(), kept: restored.kept };
      return;
    }
    const added = result.lineCount - before.lineCount;
    if (added > 0 && !atEnd()) linesBelow += added;
  }

  function control(tag: "button" | "p" | "div", className: string, action: (() => void) | null): HTMLElement {
    const element = document.createElement(tag);
    element.className = className;
    if (element instanceof HTMLButtonElement) element.type = "button";
    controls.set(element, action);
    return element;
  }

  /** The reader's own controls on rendered blocks: heading anchors, Copy, lazy Mermaid. */
  function decorate(prose: HTMLElement): void {
    for (const heading of prose.querySelectorAll<HTMLElement>("h1[id], h2[id], h3[id], h4[id]")) {
      const slug = heading.id.replace(/^user-content-/, "");
      const anchor = control("button", "h-anchor", () => void copy(`${doc.path}#${slug}`));
      anchor.setAttribute("aria-label", "Copy link to this heading");
      anchor.textContent = "#";
      heading.prepend(anchor);
    }
    for (const block of prose.querySelectorAll<HTMLElement>(".code-block")) {
      const source = block.querySelector("pre")?.textContent ?? "";
      const copyButton = control("button", "code-copy", () => void copy(source));
      copyButton.setAttribute("aria-label", "Copy code");
      const lang = document.createElement("span");
      lang.className = "lang";
      lang.textContent = block.dataset.lang ?? "";
      const action = document.createElement("span");
      action.className = "act";
      action.textContent = "Copy";
      copyButton.append(lang, action);
      block.append(copyButton);
    }
    observer?.disconnect();
    const boxes = [...prose.querySelectorAll<HTMLElement>(".mermaid")];
    const sources = new Set(boxes.map((box) => box.querySelector("code")?.textContent ?? ""));
    for (const source of diagrams.keys()) if (!sources.has(source)) diagrams.delete(source);
    if (boxes.length === 0 || typeof IntersectionObserver === "undefined") return;
    observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          observer?.unobserve(entry.target);
          void drawDiagram(entry.target as HTMLElement);
        }
      },
      { root: scroller, rootMargin: "800px 0px" },
    );
    for (const box of boxes) {
      // Sized from the source before drawing, so nothing below jumps when the diagram arrives.
      const lines = Number(box.dataset.lines ?? "1");
      box.style.height = `${Math.min(640, Math.max(120, lines * 28 + 48))}px`;
      observer.observe(box);
    }
  }

  async function drawDiagram(box: HTMLElement): Promise<void> {
    const source = box.querySelector("code")?.textContent ?? "";
    let pending = diagrams.get(source);
    if (pending === undefined) {
      pending = renderMermaid(source);
      diagrams.set(source, pending);
    }
    const result = await pending;
    if (!box.isConnected) return;
    if ("svg" in result) {
      // Cleaned SVG only (mermaid.ts): no HTML, images, links, scripts or handlers. The source
      // stays in the page, hidden, for find and copy.
      const figure = control("div", "diagram", null);
      figure.setAttribute("role", "img");
      figure.setAttribute("aria-label", "Diagram");
      figure.innerHTML = result.svg;
      box.querySelector("pre")?.setAttribute("hidden", "");
      box.append(figure);
    } else {
      box.style.height = "";
      box.classList.add("failed");
      const reason = control("p", "block-error", null);
      reason.textContent = `Mermaid · ${result.error}`;
      box.prepend(reason);
    }
  }

  /** The reader-owned control `target` is in, if any (identity, not markup). */
  function controlOf(target: Element): Element | null {
    for (let element: Element | null = target; element !== null && element !== root; element = element.parentElement) {
      if (controls.has(element)) return element;
    }
    return null;
  }

  function mark(query: string, reveal: boolean): void {
    if (content === null) return;
    if (query === "") {
      clearMatches(content);
      matches = [];
      return;
    }
    matches = markMatches(content, query, (element) => controls.has(element));
    if (matches.length === 0 || scroller === null) return;
    const base = scroller.getBoundingClientRect().top - scroller.scrollTop;
    matchIndex = firstMatchFrom(
      matches.map((hit) => hit.getBoundingClientRect().top - base),
      scroller.scrollTop,
    );
    showMatch(reveal);
  }

  function showMatch(reveal: boolean): void {
    matches.forEach((hit, index) => hit.classList.toggle("cur", index === matchIndex));
    const hit = matches[matchIndex];
    if (hit === undefined || scroller === null) return;
    if (!reveal) return;
    const base = scroller.getBoundingClientRect().top - scroller.scrollTop;
    follow = false;
    setScroll(hit.getBoundingClientRect().top - base - scroller.clientHeight / 3);
  }

  function stepMatch(direction: 1 | -1): void {
    if (matches.length === 0) return;
    matchIndex = (matchIndex + direction + matches.length) % matches.length;
    showMatch(true);
  }

  function closeFind(): void {
    findOpen = false;
    findText = "";
    scroller?.focus({ preventScroll: true });
  }

  function lead(): number {
    return content === null ? 28 : Number.parseFloat(getComputedStyle(content).lineHeight) || 28;
  }

  function goHeading(heading: { id: string }, smooth = true): void {
    const top = headingTops().find((candidate) => candidate.id === heading.id)?.top;
    if (top === undefined || scroller === null) return;
    if (top < scroller.scrollTop) follow = false;
    setScroll(top - 8, smooth);
  }

  function stepHeading(direction: 1 | -1): void {
    if (scroller === null) return;
    const position = scroller.scrollTop + 8;
    const tops = headingTops();
    const target =
      direction === 1
        ? tops.find((heading) => heading.top > position + 2)
        : tops.filter((heading) => heading.top < position - 2).at(-1);
    if (target !== undefined) goHeading(target);
  }

  function toggleFollow(): void {
    follow = !follow;
    if (follow) toEnd();
  }

  /** “copied” shows only once the clipboard actually took the text. */
  async function copy(text: string): Promise<void> {
    await run("Copy", async () => {
      await navigator.clipboard.writeText(text);
      copiedAt = Date.now();
      now = copiedAt;
    });
  }

  function outline(): void {
    if (mode === "rail") {
      if (rail?.contains(document.activeElement) === true) {
        documentSessions.railHidden = true;
        scroller?.focus({ preventScroll: true });
      } else rail?.focus();
      return;
    }
    if (documentSessions.railHidden && outlineMode(panelWidth, outlineEntries.length, false) === "rail") {
      documentSessions.railHidden = false;
      void tick().then(() => rail?.focus());
      return;
    }
    if (mode === "popover") outlineOpen = !outlineOpen;
  }

  function closeOverlays(): void {
    menuOpen = false;
    outlineOpen = false;
    footnote = null;
    scroller?.focus({ preventScroll: true });
  }

  /** Esc closes the topmost open thing: find, then the menu, then a footnote, then the outline popover. */
  function escape(): void {
    if (findOpen) closeFind();
    else if (menuOpen) menuOpen = false;
    else if (footnote !== null) footnote = null;
    else if (outlineOpen) outlineOpen = false;
    else return;
    scroller?.focus({ preventScroll: true });
  }

  /** A reading key that moved the view; reaching the end of a live file that way follows it. */
  function keyScroll(distance: number, direction: 1 | -1): void {
    if (scroller === null) return;
    setScroll(scroller.scrollTop + distance);
    follow = followAfterKeyScroll(follow, direction, atEnd(), doc.liveUntilMs > Date.now());
    if (follow) linesBelow = 0;
  }

  function perform(action: DocumentAction | { kind: DocumentMenuAction }): void {
    if (action.kind !== "follow") follow = followAfterAction(follow, action, doc.liveUntilMs > Date.now());
    switch (action.kind) {
      case "scroll": {
        const height = scroller?.clientHeight ?? 0;
        const step = action.unit === "lines" ? 3 * lead() : action.unit === "half" ? height / 2 : height - lead();
        keyScroll(step * action.direction, action.direction);
        break;
      }
      case "top":
        setScroll(0);
        break;
      case "end":
        toEnd();
        break;
      case "heading":
        stepHeading(action.direction);
        break;
      case "find-step":
        stepMatch(action.direction);
        break;
      case "follow":
        toggleFollow();
        break;
      case "outline":
        outline();
        break;
      case "find":
        if (findOpen) finder?.focus();
        else findOpen = true;
        break;
      case "neovim":
        void run("Open in Neovim", () => newNeovimForFile(itemId));
        break;
      case "reveal":
        void run("Reveal", () => revealDocument(itemId));
        break;
      case "copy-path":
        void copy(doc.path);
        break;
      case "reload":
        memory.assets.clear();
        void run("Reload", () => documentReload(itemId));
        break;
      case "menu":
        menuOpen = true;
        break;
      case "escape":
        escape();
        break;
    }
  }

  function onkeydown(event: KeyboardEvent): void {
    const target = event.target instanceof Element ? event.target : null;
    // Enter and Space activate the focused button (a reader control, the frontmatter line).
    if ((event.key === "Enter" || event.key === " ") && target instanceof HTMLButtonElement) return;
    const inRail = rail?.contains(target) === true;
    // Reading keys work from the page itself; any other control (find, the menu, the outline,
    // buttons outside the page) owns its keys. Decided by element identity, never markup.
    const reading = target === root || (scroller?.contains(target) === true && !inRail);
    // `o` pressed again from inside the rail hides it (board 34); the rail's other keys are its own.
    const plain = !event.ctrlKey && !event.metaKey && !event.altKey;
    const result =
      inRail && plain && event.key === "o"
        ? { action: { kind: "outline" } as const, pendingG: false }
        : documentKey(event, { overlay: !reading, pendingG, hasFind: findText !== "", mac });
    pendingG = result.pendingG;
    if (result.action === null) {
      if (result.pendingG) event.preventDefault();
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    perform(result.action);
  }

  function onmenuaction(action: DocumentMenuAction): void {
    menuOpen = false;
    scroller?.focus({ preventScroll: true });
    perform({ kind: action });
  }

  function onscroll(): void {
    if (scroller === null) return;
    const top = scroller.scrollTop;
    const end = atEnd();
    follow = followAfterScroll({
      follow,
      movedUp: top < lastTop - 1,
      atEnd: end,
      live: doc.liveUntilMs > Date.now(),
      programmatic,
    });
    lastTop = top;
    if (end) linesBelow = 0;
    const tops = headingTops();
    const outlineIds = new Set(outlineEntries.map((entry) => entry.id));
    currentId = tops.filter((heading) => outlineIds.has(heading.id) && heading.top <= top + 24).at(-1)?.id ?? null;
    memory.place = capturePlace(tops, top);
  }

  function onwheel(event: WheelEvent): void {
    if (event.deltaY < 0) follow = false;
  }

  function scrollToFragment(id: string): void {
    const element = document.getElementById(`user-content-${id}`) ?? document.getElementById(id);
    if (!(element instanceof HTMLElement) || content?.contains(element) !== true || scroller === null) return;
    const top = element.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
    if (top < scroller.scrollTop) follow = false;
    setScroll(top - 8, true);
  }

  /**
   * One dispatcher for every rendered subtree (the prose and the footnote preview): links never
   * navigate the privileged web view, and reader controls act by identity.
   */
  function onclick(event: MouseEvent): void {
    const target = event.target instanceof Element ? event.target : null;
    if (target === null) return;
    const link = target.closest("a");
    if (link !== null) event.preventDefault();
    const owned = controlOf(target);
    if (owned !== null) {
      controls.get(owned)?.();
      return;
    }
    const href = link?.getAttribute("href") ?? null;
    if (href === null) return;
    const destination = linkTarget(doc.path, href);
    switch (destination.kind) {
      case "fragment":
        footnote = null;
        scrollToFragment(destination.id);
        break;
      case "external":
        void run("Open link", () => documentOpenLink(itemId, destination.url));
        break;
      case "document":
        void run("Open", () => openDocument(destination.path, true));
        break;
      case "none":
        break;
    }
  }

  function onauxclick(event: MouseEvent): void {
    if (event.target instanceof Element && event.target.closest("a") !== null) event.preventDefault();
  }

  /** A remote image that does not load shows its alt text and URL (board 32). */
  function onimageerror(event: Event): void {
    const image = event.target;
    if (!(image instanceof HTMLImageElement)) return;
    const fallback = document.createElement("span");
    fallback.className = "img-fallback";
    fallback.textContent = [image.alt, image.getAttribute("src") ?? ""].filter((part) => part !== "").join(" · ");
    image.replaceWith(fallback);
  }

  function footnoteFor(target: EventTarget | null): HTMLElement | null {
    if (!(target instanceof Element)) return null;
    const ref = target.closest<HTMLElement>("a[data-footnote-ref]");
    return ref !== null && content?.contains(ref) === true ? ref : null;
  }

  function showFootnote(event: Event): void {
    const ref = footnoteFor(event.target);
    if (ref === null || root === null) return;
    const id = (ref.getAttribute("href") ?? "").slice(1);
    const note = document.getElementById(`user-content-${id}`) ?? document.getElementById(id);
    if (note === null || content?.contains(note) !== true) return;
    const clone = note.cloneNode(true) as HTMLElement;
    for (const back of clone.querySelectorAll("[data-footnote-backref]")) back.remove();
    const box = root.getBoundingClientRect();
    const at = ref.getBoundingClientRect();
    footnote = {
      html: clone.innerHTML,
      top: at.bottom - box.top + 6,
      left: Math.max(12, Math.min(at.left - box.left - 20, box.width - 372)),
    };
  }

  function hideFootnote(event: Event): void {
    if (footnoteFor(event.target) !== null) footnote = null;
  }
</script>

<!-- keydown is the reading-key router; click/auxclick route links and reader controls for every
     rendered subtree (prose and footnote preview) -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="docsurf" bind:this={root} bind:clientWidth={panelWidth} {onkeydown} {onclick} {onauxclick}>
  {#if doc.status === "missing"}
    <div class="notice" role="status">
      <span class="grow"
        ><b>No longer at this path</b><span class="muted">{relativePath} · showing the last version read</span></span
      >
      <button type="button" class="btn sm" onclick={() => perform({ kind: "reveal" })}>Reveal folder</button>
      <button type="button" class="btn sm" onclick={() => run("Close", () => itemClose(itemId, false))}>Close</button>
    </div>
  {:else if doc.status === "unreadable" || loadError !== null}
    <div class="notice err" role="alert">
      <span class="grow"
        ><b>Can't read this file</b><span class="muted">{doc.error ?? loadError ?? "Unreadable"} · {relativePath}</span
        ></span
      >
      <button type="button" class="btn sm" onclick={() => perform({ kind: "reload" })}>Retry</button>
      <button type="button" class="btn sm" onclick={() => perform({ kind: "reveal" })}>Reveal folder</button>
      <button type="button" class="btn sm" onclick={() => run("Close", () => itemClose(itemId, false))}>Close</button>
    </div>
  {:else if doc.status === "too-large"}
    <div class="notice" role="status">
      <span class="grow"
        ><b>Too large to show here</b><span class="muted"
          >{formatBytes(doc.sizeBytes)} · the reader shows files up to 8 MB · {relativePath}</span
        ></span
      >
      <button type="button" class="btn sm" onclick={() => perform({ kind: "neovim" })}>Open in Neovim</button>
      <button type="button" class="btn sm" onclick={() => perform({ kind: "reveal" })}>Reveal folder</button>
      <button type="button" class="btn sm" onclick={() => run("Close", () => itemClose(itemId, false))}>Close</button>
    </div>
  {/if}
  <!-- The scrolling page takes focus so reading keys and Tab start here. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="reader scroll"
    bind:this={scroller}
    tabindex="0"
    role="region"
    aria-label={title}
    data-sheet-autofocus
    {onscroll}
    {onwheel}
  >
    <div class="layout" class:withrail={mode === "rail"}>
      <div class="col">
        {#if rendered?.frontmatter != null}
          {@const frontmatter = rendered.frontmatter}
          <div class="fm">
            <button type="button" class="fm-line" aria-expanded={fmOpen} onclick={() => (fmOpen = !fmOpen)}
              >{fmOpen ? "▾" : "▸"} {frontmatterSummary(frontmatter)}</button
            >
            {#if fmOpen}
              <dl class="fm-fields selectable">
                {#each frontmatter.fields as field, index (index)}
                  <dt>{field.key}</dt>
                  <dd>{field.value}</dd>
                {/each}
              </dl>
            {/if}
          </div>
        {/if}
        <!-- Prose is written by apply() (sanitised HTML); hovering or focusing a footnote reference previews it. -->
        <article
          class="prose selectable"
          bind:this={content}
          onmouseover={showFootnote}
          onmouseout={hideFootnote}
          onfocusin={showFootnote}
          onfocusout={hideFootnote}
          onerrorcapture={onimageerror}
        ></article>
      </div>
      {#if mode === "rail"}
        <DocumentOutline
          bind:this={rail}
          entries={outlineEntries}
          {currentId}
          variant="rail"
          ongo={(heading: DocumentHeading) => goHeading(heading)}
          onback={() => scroller?.focus({ preventScroll: true })}
        />
      {/if}
    </div>
  </div>
  <button
    type="button"
    class="iconbtn more"
    class:on={menuOpen}
    aria-label="Document actions"
    aria-haspopup="menu"
    title="Actions (.)"
    onclick={() => (menuOpen = !menuOpen)}>⋯</button
  >
  {#if menuOpen}
    <DocumentMenu {follow} onaction={onmenuaction} onclose={closeOverlays} />
  {/if}
  {#if findOpen}
    <DocumentFind
      bind:this={finder}
      bind:query={findText}
      current={matchIndex}
      total={matches.length}
      onstep={stepMatch}
      onclose={closeFind}
    />
  {/if}
  {#if outlineOpen && mode === "popover"}
    <DocumentOutline
      entries={outlineEntries}
      {currentId}
      variant="popover"
      ongo={(heading: DocumentHeading) => {
        outlineOpen = false;
        goHeading(heading);
        scroller?.focus({ preventScroll: true });
      }}
      onback={closeOverlays}
    />
  {/if}
  {#if footnote !== null}
    <div class="menu fn-pop selectable" role="note" style:top="{footnote.top}px" style:left="{footnote.left}px">
      <!-- eslint-disable-next-line svelte/no-at-html-tags -- a clone of the sanitised footnote already in the page -->
      {@html footnote.html}
    </div>
  {/if}
  {#if shownLine !== null}
    <button
      type="button"
      class="line"
      class:quiet={line === null || !line.visible}
      tabindex="-1"
      aria-label={shownLine.clickable ? `${shownLine.text}. Toggle follow` : shownLine.text}
      onclick={shownLine.clickable ? toggleFollow : undefined}>{shownLine.text}</button
    >
  {/if}
</div>

<style>
  .docsurf {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    position: relative;
    background: var(--panel);
    --doc-measure: 560px;
    --doc-body: 17px;
    --doc-lead: 28px;
    --doc-gap: 20px;
    --doc-top: 56px;
    --cyan: #89ddff;
  }
  /* Small window (board 32): 15/24 at a 490 px measure. */
  @media (max-width: 799px) {
    .docsurf {
      --doc-measure: 490px;
      --doc-body: 15px;
      --doc-lead: 24px;
      --doc-gap: 16px;
      --doc-top: 24px;
    }
  }
  .notice {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--line);
    background: var(--well);
    font-size: 12.5px;
    color: var(--hi);
    flex: none;
  }
  .notice.err b {
    color: var(--err-text);
  }
  .notice .grow {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .notice .muted {
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .reader {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .reader:focus-visible {
    outline: none;
  }
  .layout {
    display: flex;
    align-items: flex-start;
    min-height: 100%;
  }
  .col {
    flex: 1;
    min-width: 0;
    padding: var(--doc-top) 24px 96px;
  }
  .fm,
  .prose {
    max-width: var(--doc-measure);
    margin: 0 auto;
  }
  .fm {
    margin-bottom: 28px;
    font-family: var(--mono);
    font-size: 12.5px;
    color: var(--dim);
  }
  .fm-line {
    line-height: 20px;
    color: var(--dim);
    border-radius: 4px;
  }
  .fm-line:hover {
    color: var(--text);
  }
  .fm-fields {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 2px 16px;
    margin: 8px 0 0 14px;
    line-height: 20px;
  }
  .fm-fields dt {
    color: var(--dim);
  }
  .fm-fields dd {
    margin: 0;
    color: var(--text);
    overflow-wrap: anywhere;
  }
  .more {
    position: absolute;
    top: 10px;
    right: 12px;
    z-index: 3;
    font-size: 16px;
    opacity: 0;
  }
  .docsurf:hover .more,
  .docsurf:focus-within .more,
  .more.on {
    opacity: 1;
  }
  .fn-pop {
    width: 360px;
    padding: 10px 12px;
    font-size: 14px;
    line-height: 22px;
    color: var(--text);
    max-height: 240px;
  }
  .fn-pop :global(p) {
    margin: 0;
  }
  .line {
    position: absolute;
    right: 16px;
    bottom: 10px;
    z-index: 2;
    font-size: 12px;
    line-height: 18px;
    padding: 2px 8px;
    border-radius: 6px;
    color: var(--dim);
    background: var(--panel);
    opacity: 1;
    transition: opacity 600ms ease;
  }
  .line:hover {
    color: var(--text);
  }
  .line.quiet {
    opacity: 0;
    pointer-events: none;
  }
  @media (prefers-reduced-motion: reduce) {
    .line {
      transition: none;
    }
  }

  /* ---------- prose: board 31's type scale ---------- */
  .prose {
    font-family: var(--ui);
    font-size: var(--doc-body);
    line-height: var(--doc-lead);
    color: var(--text);
    overflow-wrap: break-word;
  }
  .prose :global(> :first-child) {
    margin-top: 0;
  }
  .prose :global(p),
  .prose :global(ul),
  .prose :global(ol),
  .prose :global(dl) {
    margin: 0 0 var(--doc-gap);
  }
  .prose :global(strong) {
    color: var(--hi);
    font-weight: 600;
  }
  .prose :global(h1),
  .prose :global(h2),
  .prose :global(h3),
  .prose :global(h4),
  .prose :global(h5),
  .prose :global(h6) {
    position: relative;
    color: var(--hi);
    font-weight: 600;
  }
  .prose :global(h1) {
    font-size: 32px;
    line-height: 40px;
    letter-spacing: -0.015em;
    margin: 0 0 16px;
  }
  .prose :global(h2) {
    font-size: 23px;
    line-height: 32px;
    letter-spacing: -0.01em;
    margin: 52px 0 12px;
  }
  .prose :global(h3) {
    font-size: 18px;
    line-height: 28px;
    margin: 36px 0 8px;
  }
  .prose :global(h4),
  .prose :global(h5),
  .prose :global(h6) {
    font-size: 13px;
    line-height: 20px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--dim);
    margin: 28px 0 6px;
  }
  @media (max-width: 799px) {
    .prose :global(h1) {
      font-size: 23px;
      line-height: 30px;
    }
    .prose :global(h2) {
      font-size: 18px;
      line-height: 26px;
    }
  }
  .prose :global(.sr-only) {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
  .prose :global(.h-anchor) {
    position: absolute;
    left: -22px;
    top: 0;
    width: 18px;
    color: var(--dim);
    font-weight: 400;
    opacity: 0;
  }
  /* Shown on hover, and while the anchor itself has keyboard focus. */
  .prose :global(h1:hover .h-anchor),
  .prose :global(h2:hover .h-anchor),
  .prose :global(h3:hover .h-anchor),
  .prose :global(h4:hover .h-anchor),
  .prose :global(.h-anchor:focus-visible) {
    opacity: 1;
  }
  .prose :global(a) {
    color: var(--cyan);
    text-decoration: underline;
    text-decoration-color: #4a6b78;
    text-decoration-thickness: 1px;
    text-underline-offset: 3px;
    cursor: pointer;
  }
  .prose :global(a:hover) {
    text-decoration-color: var(--cyan);
  }
  .prose :global(ul),
  .prose :global(ol) {
    padding-left: 24px;
  }
  .prose :global(li + li) {
    margin-top: 4px;
  }
  .prose :global(li > ul),
  .prose :global(li > ol) {
    margin: 4px 0 0;
  }
  .prose :global(.contains-task-list) {
    list-style: none;
    padding-left: 4px;
  }
  .prose :global(.task-list-item input) {
    margin: 0 8px 0 0;
    vertical-align: -1px;
  }
  .prose :global(hr) {
    border: 0;
    border-top: 1px solid var(--line);
    margin: 32px 0;
  }
  .prose :global(blockquote) {
    margin: 24px 0;
    padding: 0 0 0 18px;
    border-left: 2px solid var(--outline);
    color: var(--text);
  }
  .prose :global(blockquote > :last-child) {
    margin-bottom: 0;
  }
  .prose :global(.callout-label) {
    font-size: 12px;
    line-height: 20px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    margin: 0 0 4px;
    color: var(--dim);
  }
  .prose :global(.callout-tip) {
    border-left-color: var(--green);
  }
  .prose :global(.callout-tip .callout-label) {
    color: var(--green);
  }
  .prose :global(.callout-warning),
  .prose :global(.callout-important) {
    border-left-color: var(--yellow);
  }
  .prose :global(.callout-warning .callout-label),
  .prose :global(.callout-important .callout-label) {
    color: var(--yellow);
  }
  .prose :global(.callout-caution) {
    border-left-color: var(--err);
  }
  .prose :global(.callout-caution .callout-label) {
    color: var(--err-text);
  }
  .prose :global(code) {
    font-family: var(--mono);
  }
  .prose :global(:not(pre) > code) {
    font-size: 0.84em;
    color: var(--hi);
    background: #2b2b2b;
    border-radius: 4px;
    padding: 1px 4px;
  }
  .prose :global(.code-block) {
    position: relative;
    margin: 24px -20px;
  }
  .prose :global(.code-block pre),
  .prose :global(.mermaid pre) {
    margin: 0;
    padding: 14px 20px;
    background: var(--well);
    border-radius: 6px;
    font-family: var(--mono);
    font-size: 13.5px;
    line-height: 22px;
    color: var(--fg);
    overflow-x: auto;
    scrollbar-width: thin;
    scrollbar-color: #3a3a3a transparent;
  }
  .prose :global(.code-copy) {
    position: absolute;
    top: 6px;
    right: 10px;
    font-size: 11.5px;
    line-height: 16px;
    padding: 1px 6px;
    border-radius: 4px;
    color: var(--dim);
    font-family: var(--ui);
  }
  .prose :global(.code-copy .act),
  .prose :global(.code-block:hover .code-copy .lang),
  .prose :global(.code-block:focus-within .code-copy .lang) {
    display: none;
  }
  .prose :global(.code-block:hover .code-copy .act),
  .prose :global(.code-block:focus-within .code-copy .act) {
    display: inline;
  }
  .prose :global(.code-copy:hover) {
    color: var(--hi);
    background: var(--hover);
  }
  .prose :global(.mermaid) {
    margin: 28px 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    justify-content: center;
  }
  .prose :global(.mermaid.failed) {
    margin: 24px -20px;
  }
  .prose :global(.diagram) {
    height: 100%;
    display: flex;
    justify-content: center;
  }
  .prose :global(.diagram svg) {
    max-width: 100%;
    max-height: 100%;
    height: auto;
  }
  .prose :global(.block-error) {
    font-size: 12.5px;
    line-height: 20px;
    color: var(--err-text);
    margin: 0 20px 6px;
  }
  .prose :global(.katex-display) {
    margin: 28px 0;
    overflow-x: auto;
    overflow-y: hidden;
  }
  .prose :global(.katex-error) {
    font-family: var(--mono);
    color: var(--err-text);
  }
  .prose :global(img) {
    max-width: 100%;
    height: auto;
    border-radius: 4px;
  }
  .prose :global(.img-fallback) {
    font-size: 13.5px;
    color: var(--dim);
    font-style: italic;
    overflow-wrap: anywhere;
  }
  .prose :global(table) {
    border-collapse: collapse;
    margin: 24px 0;
    font-size: 14.5px;
    line-height: 22px;
    display: block;
    overflow-x: auto;
    max-width: 100%;
  }
  .prose :global(th) {
    color: var(--hi);
    font-weight: 600;
    text-align: left;
    border-bottom: 1px solid var(--outline);
    padding: 6px 16px 6px 0;
  }
  .prose :global(td) {
    border-bottom: 1px solid var(--line);
    padding: 6px 16px 6px 0;
  }
  .prose :global(td.num) {
    font-family: var(--mono);
    font-size: 13.5px;
    text-align: right;
  }
  .prose :global(.footnotes) {
    margin-top: 44px;
    padding-top: 16px;
    border-top: 1px solid var(--line);
    font-size: 14px;
    line-height: 22px;
    color: #9aa7ad;
  }
  .prose :global(.footnotes p) {
    margin: 0 0 8px;
  }
  .prose :global(sup a) {
    text-decoration: none;
  }
  .prose :global(mark.find-hit) {
    background: #5e4c22;
    color: inherit;
    border-radius: 2px;
  }
  .prose :global(mark.find-hit.cur) {
    box-shadow: 0 0 0 1px var(--yellow);
  }
</style>
