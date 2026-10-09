<script lang="ts">
  // A number setting: edits a local draft, writes on Enter or blur (native validates the range).
  let {
    value,
    unit,
    label,
    min,
    max,
    step = 1,
    disabled = false,
    oncommit,
  }: {
    value: number | undefined;
    unit: string;
    label: string;
    min: number;
    max: number;
    step?: number;
    disabled?: boolean;
    oncommit: (value: number) => void;
  } = $props();

  let draft = $state<string | null>(null);

  function commit(): void {
    if (draft === null) return;
    const text = draft.trim();
    draft = null;
    const next = Number(text);
    if (text === "" || !Number.isFinite(next) || next === value) return;
    oncommit(next);
  }
</script>

<label class="field num">
  <input
    type="number"
    inputmode="decimal"
    aria-label={label}
    {min}
    {max}
    {step}
    {disabled}
    value={draft ?? (value === undefined ? "" : String(value))}
    oninput={(event) => (draft = event.currentTarget.value)}
    onblur={commit}
    onkeydown={(event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        commit();
      } else if (event.key === "Escape" && draft !== null) {
        event.preventDefault();
        event.stopPropagation();
        draft = null;
      }
    }}
  />
  <span class="unit">{unit}</span>
</label>

<style>
  .num {
    width: 92px;
    height: 26px;
    padding: 0 8px;
  }
  .num input {
    text-align: right;
    -moz-appearance: textfield;
    appearance: textfield;
  }
  .num input::-webkit-outer-spin-button,
  .num input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }
  .unit {
    color: #6b6b6b;
    font-size: 12px;
  }
</style>
