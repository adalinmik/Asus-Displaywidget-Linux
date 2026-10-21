<script setup lang="ts">
import { computed, ref, watchEffect } from "vue";
import type { Setting } from "./api";
import Dropdown from "./Dropdown.vue";
import Icon, { type IconName } from "./Icon.vue";

const props = defineProps<{ setting: Setting; disabled: boolean; icon?: IconName }>();
const emit = defineEmits<{ change: [value: string] }>();

// Controls move freely and snap back to the monitor's value whenever a new reading arrives.
const level = ref(0);
const on = ref(false);
watchEffect(() => {
  const s = props.setting;
  level.value = s.current ?? 0;
  on.value = s.on ?? false;
});

const pct = computed(() => `${props.setting.max ? (level.value / props.setting.max) * 100 : 0}%`);
// Short option lists read better as a segmented control; long ones fall back to a dropdown.
const segmented = computed(() => {
  const s = props.setting;
  return s.kind === "enum" && s.choices.length <= 4 && s.choices.every((c) => c.label.length <= 12);
});
// Up/down/left/right actions (OSD keys, GamePlus position) render as one D-pad; Enter is its centre.
const DIRECTIONS = ["up", "down", "left", "right"];
const dpad = computed(() => props.setting.kind === "action" && DIRECTIONS.every((d) => props.setting.choices.some((c) => c.name === d)));
const center = computed(() => props.setting.choices.find((c) => c.name === "enter"));
const extra = computed(() => props.setting.choices.filter((c) => !DIRECTIONS.includes(c.name) && c.name !== "enter"));
const label = (name: string) => props.setting.choices.find((c) => c.name === name)?.label ?? name;

function padKey(e: KeyboardEvent) {
  const keys: Record<string, string | undefined> = {
    ArrowUp: "up",
    ArrowDown: "down",
    ArrowLeft: "left",
    ArrowRight: "right",
    Enter: center.value?.name,
  };
  const name = keys[e.key];
  if (!name || props.disabled) return;
  e.preventDefault();
  emit("change", name);
}

const stacked = computed(() => !props.setting.error && !props.setting.unavailable && (props.setting.kind === "continuous" || segmented.value));
</script>

<template>
  <div class="row" :class="{ stacked, disabled: setting.error || setting.unavailable }" :title="setting.note ?? undefined">
    <template v-if="setting.error">
      <label>{{ setting.label }}</label>
      <span class="muted err" :title="setting.error"><Icon name="alert-circle" />Couldn't read</span>
    </template>

    <template v-else-if="setting.unavailable">
      <label>{{ setting.label }}</label>
      <span class="muted"><Icon name="ban" />Not available in this mode</span>
    </template>

    <template v-else-if="setting.kind === 'continuous'">
      <div class="row-head">
        <label :for="setting.key"><Icon v-if="icon" :name="icon" />{{ setting.label }}</label>
        <output :for="setting.key">{{ level }}</output>
      </div>
      <input
        :id="setting.key"
        v-model.number="level"
        type="range"
        min="0"
        :max="setting.max"
        :style="{ '--pct': pct }"
        :disabled="disabled"
        @change="emit('change', String(level))"
      />
    </template>

    <template v-else-if="segmented">
      <div class="row-head">
        <label>{{ setting.label }}</label>
        <Icon v-if="setting.force" name="alert" class="warn-icon" />
      </div>
      <div class="segmented" role="radiogroup" :aria-label="setting.label">
        <button
          v-for="c in setting.choices"
          :key="c.name"
          role="radio"
          :aria-checked="c.name === setting.selected"
          :class="{ on: c.name === setting.selected }"
          :disabled="disabled"
          @click="c.name !== setting.selected && emit('change', c.name)"
        >
          {{ c.label }}
        </button>
      </div>
    </template>

    <template v-else-if="setting.kind === 'enum'">
      <label :for="setting.key">{{ setting.label }}</label>
      <Dropdown
        :id="setting.key"
        :choices="setting.choices"
        :selected="setting.selected"
        :placeholder="setting.text ?? ''"
        :disabled="disabled"
        @change="emit('change', $event)"
      />
    </template>

    <template v-else-if="setting.kind === 'toggle'">
      <label :for="setting.key">{{ setting.label }}</label>
      <input
        :id="setting.key"
        v-model="on"
        type="checkbox"
        role="switch"
        :disabled="disabled"
        @change="emit('change', on ? 'on' : 'off')"
      />
    </template>

    <template v-else-if="dpad">
      <label>{{ setting.label }}</label>
      <div class="pad-wrap">
        <div v-if="extra.length" class="pad-keys">
          <button v-for="c in extra" :key="c.name" :disabled="disabled" @click="emit('change', c.name)">{{ c.label }}</button>
        </div>
        <div class="dpad" role="group" :aria-label="setting.label" @keydown="padKey">
          <button
            v-for="d in DIRECTIONS"
            :key="d"
            :class="d"
            :aria-label="label(d)"
            :title="label(d)"
            :disabled="disabled"
            @click="emit('change', d)"
          >
            <Icon name="chevron" />
          </button>
          <button v-if="center" class="center" :aria-label="center.label" :title="center.label" :disabled="disabled" @click="emit('change', center.name)">
            OK
          </button>
        </div>
      </div>
    </template>

    <template v-else-if="setting.kind === 'action'">
      <label>{{ setting.label }}</label>
      <div class="actions">
        <button v-for="c in setting.choices" :key="c.name" :disabled="disabled" @click="emit('change', c.name)">
          <Icon name="reset" />{{ c.label }}
        </button>
      </div>
    </template>

    <template v-else>
      <label>{{ setting.label }}</label>
      <span :id="setting.key" class="info">{{ setting.text }}</span>
    </template>
  </div>
</template>
