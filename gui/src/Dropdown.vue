<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref } from "vue";
import type { Choice } from "./api";
import Icon from "./Icon.vue";

// Stands in for <select>: WebKitGTK opens a native <select> as a GTK popup, which Wayland
// compositors refuse to map ("Tried to map a popup with a non-top most parent").
const props = defineProps<{ id: string; choices: Choice[]; selected?: string; placeholder: string; disabled: boolean }>();
const emit = defineEmits<{ change: [value: string] }>();

const open = ref(false);
const active = ref(0);
const button = ref<HTMLButtonElement>();
const list = ref<HTMLUListElement>();
const place = ref<Record<string, string>>({});

const current = computed(() => props.choices.find((c) => c.name === props.selected));

function position() {
  const r = button.value!.getBoundingClientRect();
  const below = window.innerHeight - r.bottom - 8;
  const up = below < 200 && r.top > below;
  place.value = {
    left: `${r.left}px`,
    minWidth: `${r.width}px`,
    maxHeight: `${Math.min(300, (up ? r.top : below) - 8)}px`,
    ...(up ? { bottom: `${window.innerHeight - r.top + 4}px` } : { top: `${r.bottom + 4}px` }),
  };
}

async function show() {
  if (props.disabled || open.value) return;
  active.value = Math.max(0, props.choices.findIndex((c) => c.name === props.selected));
  position();
  open.value = true;
  document.addEventListener("pointerdown", outside, true);
  window.addEventListener("resize", close);
  document.addEventListener("scroll", reposition, true);
  await nextTick();
  list.value?.focus();
  scrollToActive();
}

function close() {
  if (!open.value) return;
  open.value = false;
  document.removeEventListener("pointerdown", outside, true);
  window.removeEventListener("resize", close);
  document.removeEventListener("scroll", reposition, true);
}

function reposition(e: Event) {
  if (e.target !== list.value) position();
}

function outside(e: PointerEvent) {
  const t = e.target as Node;
  if (!list.value?.contains(t) && !button.value?.contains(t)) close();
}

function pick(i: number) {
  const c = props.choices[i];
  close();
  button.value?.focus();
  if (c && c.name !== props.selected) emit("change", c.name);
}

function scrollToActive() {
  list.value?.children[active.value]?.scrollIntoView({ block: "nearest" });
}

let typed = "";
let typedAt = 0;
function key(e: KeyboardEvent) {
  const last = props.choices.length - 1;
  const moves: Record<string, number> = {
    ArrowDown: Math.min(last, active.value + 1),
    ArrowUp: Math.max(0, active.value - 1),
    Home: 0,
    End: last,
    PageDown: Math.min(last, active.value + 8),
    PageUp: Math.max(0, active.value - 8),
  };
  if (e.key in moves) {
    active.value = moves[e.key];
    scrollToActive();
  } else if (e.key === "Enter" || e.key === " ") {
    pick(active.value);
  } else if (e.key === "Escape" || e.key === "Tab") {
    close();
    button.value?.focus();
  } else if (e.key.length === 1) {
    // Type-ahead, like a native select.
    typed = e.timeStamp - typedAt < 700 ? typed + e.key.toLowerCase() : e.key.toLowerCase();
    typedAt = e.timeStamp;
    const i = props.choices.findIndex((c) => c.label.toLowerCase().startsWith(typed));
    if (i >= 0) {
      active.value = i;
      scrollToActive();
    }
  } else {
    return;
  }
  e.preventDefault();
}

function buttonKey(e: KeyboardEvent) {
  if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
    e.preventDefault();
    show();
  }
}

onBeforeUnmount(close);
</script>

<template>
  <div class="select">
    <button
      :id="id"
      ref="button"
      type="button"
      class="select-button"
      aria-haspopup="listbox"
      :aria-expanded="open"
      :disabled="disabled"
      @click="open ? close() : show()"
      @keydown="buttonKey"
    >
      <span>{{ current?.label ?? placeholder }}</span>
      <Icon name="chevron" />
    </button>
    <Teleport to="body">
      <ul
        v-if="open"
        ref="list"
        class="select-list"
        role="listbox"
        tabindex="-1"
        :aria-activedescendant="`${id}-opt-${active}`"
        :style="place"
        @keydown="key"
      >
        <li
          v-for="(c, i) in choices"
          :id="`${id}-opt-${i}`"
          :key="c.name"
          role="option"
          :aria-selected="c.name === selected"
          :class="{ active: i === active, selected: c.name === selected }"
          @pointerenter="active = i"
          @click="pick(i)"
        >
          {{ c.label }}
        </li>
      </ul>
    </Teleport>
  </div>
</template>
