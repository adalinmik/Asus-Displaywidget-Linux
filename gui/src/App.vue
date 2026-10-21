<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { getName, getTauriVersion, getVersion } from "@tauri-apps/api/app";
import * as api from "./api";
import type { Setting, Unsupported } from "./api";
import SettingRow from "./SettingRow.vue";
import Icon, { type IconName } from "./Icon.vue";

const dev = import.meta.env.DEV;

const monitors = ref<string[]>([]);
const active = ref(0);
const settings = ref<Setting[]>([]);
const unsupported = ref<Unsupported[]>([]);
const autostart = ref(false);
const loading = ref(true);
const busy = ref(false);
const error = ref("");
const section = ref("");
// The main area shows either the active monitor or the app's own page (Settings, About).
const view = ref<"monitor" | "app">("monitor");

// Icon, colour and subtitle per backend group name. Order matters: GamePlus before Gaming.
const SECTIONS: { match: RegExp; icon: IconName; tone: string; sub: string }[] = [
  { match: /^display/i, icon: "monitor", tone: "#60a5fa", sub: "Resolution, refresh rate and size" },
  { match: /^picture/i, icon: "sun", tone: "#38bdf8", sub: "Luminance and clarity" },
  { match: /colou?r/i, icon: "palette", tone: "#8b5cf6", sub: "Temperature and gain" },
  { match: /hdr/i, icon: "contrast", tone: "#f59e0b", sub: "High dynamic range" },
  { match: /gameplus/i, icon: "game", tone: "#d946ef", sub: "Crosshair, timer, counter" },
  { match: /gam/i, icon: "game", tone: "#d946ef", sub: "Response time and sync" },
  { match: /oled/i, icon: "shield", tone: "#34d399", sub: "Burn-in protection" },
  { match: /pip/i, icon: "pip", tone: "#22d3ee", sub: "Picture-in-picture" },
  { match: /osd/i, icon: "sliders", tone: "#a78bfa", sub: "On-screen keys and shortcuts" },
  { match: /system/i, icon: "plug", tone: "#fbbf24", sub: "Input, power and language" },
  { match: /info/i, icon: "info", tone: "#94a3b8", sub: "Firmware and version" },
  // App sections, after the monitor's.
  { match: /^settings$/i, icon: "sliders", tone: "#94a3b8", sub: "App preferences" },
  { match: /^about$/i, icon: "info", tone: "#94a3b8", sub: "Version and build" },
];
const APP_SECTIONS = ["Settings", "About"];
const meta = (group: string) => SECTIONS.find((s) => s.match.test(group)) ?? { icon: "sliders" as IconName, tone: "#94a3b8", sub: "" };
const slug = (group: string) => "sec-" + group.toLowerCase().replace(/[^a-z0-9]+/g, "-");

// Brightness and contrast get pulled up into the live display card, and out of the Picture card.
const HERO_KEYS = ["brightness", "contrast"];
const quick = computed(() =>
  HERO_KEYS.map((key) => settings.value.find((s) => s.key === key)).filter((s): s is Setting => !!s && !s.error && !s.unavailable),
);

const groups = computed(() => {
  const inHero = new Set(quick.value.map((s) => s.key));
  const groups = new Map<string, Setting[]>();
  for (const s of settings.value) {
    if (inHero.has(s.key)) continue;
    groups.set(s.group, [...(groups.get(s.group) ?? []), s]);
  }
  return groups;
});

const status = computed(() =>
  !monitors.value.length && !loading.value ? { text: "Not detected", cls: "off" } : busy.value ? { text: "Syncing…", cls: "warn" } : { text: "Connected", cls: "" },
);

// Dev builds: how long the last monitor request took, shown next to the status.
const lastSync = ref<number>();

async function request(task: () => Promise<unknown>) {
  busy.value = true;
  const start = performance.now();
  try {
    await task();
  } catch (e) {
    error.value = String(e);
  } finally {
    busy.value = false;
    lastSync.value = performance.now() - start;
  }
}

// Dev builds: the app's own frame rate, counted over each second.
const fps = ref<number>();
let fpsFrame = 0;
function countFrames() {
  let frames = 0;
  let since = performance.now();
  const tick = (now: number) => {
    frames++;
    if (now - since >= 1000) {
      fps.value = Math.round((frames * 1000) / (now - since));
      frames = 0;
      since = now;
    }
    fpsFrame = requestAnimationFrame(tick);
  };
  fpsFrame = requestAnimationFrame(tick);
}

const perf = computed(() => {
  const parts = [];
  if (fps.value !== undefined) parts.push(`${fps.value} fps`);
  if (lastSync.value !== undefined) parts.push(`sync ${lastSync.value < 1000 ? `${Math.round(lastSync.value)} ms` : `${(lastSync.value / 1000).toFixed(1)} s`}`);
  return parts.join(" · ");
});

async function load() {
  loading.value = true;
  await request(async () => {
    settings.value = monitors.value.length ? await api.readSettings(active.value) : [];
    unsupported.value = dev && monitors.value.length ? await api.listUnsupported(active.value) : [];
  });
  loading.value = false;
}

// Settings per monitor, kept when switching away so switching back shows them at once (no DDC/CI
// reads). Changes, reloads and live values update the entry; Refresh starts over.
const cache = new Map<number, { settings: Setting[]; unsupported: Unsupported[] }>();
watch([settings, unsupported], ([s, u]) => {
  if (s.length) cache.set(active.value, { settings: s, unsupported: u });
});

// The other monitors are read in the background once the first one shows, so switching is instant
// from the start. Each monitor has its own lock in the backend, so this doesn't hold up the active
// one. `generation` drops replies that belong to a monitor list Refresh has replaced.
const prefetching = new Map<number, Promise<void>>();
let generation = 0;
function prefetch(i: number) {
  if (cache.has(i)) return;
  if (prefetching.has(i)) return prefetching.get(i);
  const gen = generation;
  const task = (async () => {
    try {
      const s = await api.readSettings(i);
      const u = dev ? await api.listUnsupported(i) : [];
      if (gen === generation && !cache.has(i)) cache.set(i, { settings: s, unsupported: u });
    } catch {
      // Picking the monitor reads it again and shows the error.
    } finally {
      if (gen === generation) prefetching.delete(i);
    }
  })();
  prefetching.set(i, task);
  return task;
}

async function refresh() {
  error.value = "";
  cache.clear();
  prefetching.clear();
  generation++;
  loading.value = true;
  await request(async () => {
    monitors.value = await api.listMonitors();
    active.value = 0;
  });
  await load();
  monitors.value.forEach((_, i) => i !== active.value && prefetch(i));
}

async function pick(i: number) {
  if (busy.value && i !== active.value) return;
  view.value = "monitor";
  if (i === active.value) return;
  active.value = i;
  // Still being prefetched: wait for that read instead of starting a second one.
  const pending = prefetching.get(i);
  if (pending && !cache.has(i)) {
    settings.value = [];
    unsupported.value = [];
    loading.value = true;
    await pending;
    loading.value = false;
    if (active.value !== i) return;
  }
  const cached = cache.get(i);
  if (cached) {
    settings.value = cached.settings;
    unsupported.value = cached.unsupported;
    return;
  }
  settings.value = [];
  unsupported.value = [];
  load();
}

// Confirmation for settings that need force (input switch, reset).
const dialog = ref<HTMLDialogElement>();
const pending = ref<Setting>();
let resolveConfirm: (ok: boolean) => void = () => {};
function ask(setting: Setting) {
  pending.value = setting;
  dialog.value?.showModal();
  return new Promise<boolean>((r) => (resolveConfirm = r));
}
function answer(ok: boolean) {
  dialog.value?.close();
  resolveConfirm(ok);
}

async function change(setting: Setting, value: string) {
  error.value = "";
  // A fresh copy resets the control if nothing changes.
  let next: Setting = { ...setting };
  const confirmed = !setting.force || (await ask(setting));
  if (confirmed) {
    await request(async () => {
      next = await api.changeSetting(active.value, setting.key, value, setting.force);
    });
  }
  settings.value = settings.value.map((s) => (s.key === next.key ? next : s));

  // A new mode, preset, input etc. changes other settings too.
  if (confirmed && !error.value && setting.reload) await load();
}

async function toggleAutostart(enabled: boolean) {
  error.value = "";
  await request(() => api.setAutostart(enabled));
  autostart.value = await api.autostartEnabled();
}

const mainEl = ref<HTMLElement>();
// A section in the other view switches to it first.
async function go(name: string) {
  const target = APP_SECTIONS.includes(name) ? "app" : "monitor";
  if (view.value !== target) {
    view.value = target;
    await nextTick();
  }
  section.value = name;
  document.getElementById(slug(name))?.scrollIntoView({ block: "start" });
}
// Each view starts at its top.
watch(view, () => nextTick(() => mainEl.value?.scrollTo({ top: 0 })));
function onScroll() {
  const top = mainEl.value?.getBoundingClientRect().top ?? 0;
  for (const g of view.value === "app" ? APP_SECTIONS : groups.value.keys()) {
    const el = document.getElementById(slug(g));
    if (el && el.getBoundingClientRect().top - top < 140) section.value = g;
  }
}

// Dev builds: Refresh Rate follows the monitor live (whether it tracks the frame rate under VRR is
// untested). One read a second, skipped while the window is hidden or another request holds the
// monitor.
const LIVE = ["refresh-rate"];
let liveTimer: ReturnType<typeof setInterval> | undefined;
let liveBusy = false;
async function pollLive() {
  if (liveBusy || busy.value || loading.value || document.hidden || view.value !== "monitor" || !monitors.value.length) return;
  const index = active.value;
  const keys = LIVE.filter((k) => settings.value.some((s) => s.key === k));
  if (!keys.length) return;
  liveBusy = true;
  try {
    for (const key of keys) {
      const next = await api.readSetting(index, key);
      // Drop the reply if the user switched monitors or started a request meanwhile.
      if (index !== active.value || busy.value || loading.value) return;
      settings.value = settings.value.map((s) => (s.key === key ? next : s));
    }
  } catch {
    // A missed read is fine; the next tick tries again.
  } finally {
    liveBusy = false;
  }
}

// About card.
const about = ref<{ name: string; version: string; tauri: string }>();
const REPOSITORY = "https://github.com/KharalDipendra/Asus-Displaywidget-Linux";

onMounted(async () => {
  about.value = { name: await getName(), version: await getVersion(), tauri: await getTauriVersion() };
  autostart.value = await api.autostartEnabled();
  await refresh();
  if (dev) {
    liveTimer = setInterval(pollLive, 1000);
    countFrames();
  }
});
onBeforeUnmount(() => {
  clearInterval(liveTimer);
  cancelAnimationFrame(fpsFrame);
});
</script>

<template>
  <div class="shell">
    <aside class="sidebar">
      <div class="brand">
        <img src="/icon.png" alt="" />
        <div>
          <h1>AsusDisplay</h1>
          <p>DDC/CI monitor control</p>
        </div>
      </div>

      <div class="side-group">
        <div class="side-label">
          Monitors
          <span class="side-count"><span class="dot" :class="status.cls"></span>{{ monitors.length ? `${monitors.length} detected` : "none detected" }}</span>
        </div>
        <button
          v-for="(name, i) in monitors"
          :key="i"
          class="monitor-btn"
          :class="{ active: view === 'monitor' && i === active }"
          :disabled="busy && i !== active"
          @click="pick(i)"
        >
          <span class="tile"><Icon name="monitor" /></span>
          <span>{{ name }}</span>
        </button>
      </div>

      <nav v-if="settings.length" class="side-group">
        <div class="side-label">Sections</div>
        <button v-for="[group] in groups" :key="group" class="nav-btn" :class="{ active: view === 'monitor' && section === group }" @click="go(group)">
          <Icon :name="meta(group).icon" />{{ group }}
        </button>
      </nav>

      <nav class="side-group">
        <div class="side-label">App</div>
        <button v-for="name in APP_SECTIONS" :key="name" class="nav-btn" :class="{ active: view === 'app' && section === name }" @click="go(name)">
          <Icon :name="meta(name).icon" />{{ name }}
        </button>
      </nav>

    </aside>

    <div class="content">
      <header class="topbar">
        <div v-if="view === 'app'" class="topbar-title">
          <small>App</small>
          <h2>AsusDisplay</h2>
        </div>
        <div v-else class="topbar-title">
          <small v-if="monitors.length">Monitor {{ active + 1 }} of {{ monitors.length }}</small>
          <h2>{{ monitors[active] ?? "AsusDisplay" }}</h2>
        </div>
        <span class="status">
          <span class="dot" :class="status.cls"></span>{{ status.text }}
          <span v-if="dev && perf" class="perf" title="Dev build: app frame rate and the last monitor request's duration">{{ perf }}</span>
        </span>
        <button v-if="view === 'monitor'" class="btn" :disabled="busy" @click="refresh"><Icon name="refresh" />Refresh</button>
        <div class="progress" :class="{ active: busy }"></div>
      </header>

      <main ref="mainEl" @scroll.passive="onScroll">
        <div class="stack">
          <p v-if="error" class="error" role="alert">
            <Icon name="alert" /><span>{{ error }}</span>
            <button aria-label="Dismiss" @click="error = ''"><Icon name="x" /></button>
          </p>

          <template v-if="view === 'monitor'">
            <p v-if="loading && !settings.length" class="hint">Reading monitor settings…</p>

            <div v-else-if="!monitors.length" class="empty">
              <span class="tile"><Icon name="monitor-off" /></span>
              <h3>No monitors found</h3>
              <p>Check that the i2c-dev module is loaded and the udev rule is installed. See the README for setup.</p>
              <code>sudo modprobe i2c-dev</code>
            </div>

            <template v-else>
              <section class="hero">
                <div class="hero-id">
                  <img src="/icon.png" alt="" />
                  <div>
                    <div class="eyebrow">Live display</div>
                    <h3>{{ monitors[active] }}</h3>
                  </div>
                </div>
                <div v-if="quick.length" class="hero-controls">
                  <SettingRow
                    v-for="(s, i) in quick"
                    :key="s.key"
                    :setting="s"
                    :icon="i === 0 ? 'sun' : 'contrast'"
                    :disabled="busy"
                    @change="change(s, $event)"
                  />
                </div>
              </section>

              <!-- A new grid per monitor: WebKit doesn't always re-flow CSS columns when their cards change. -->
              <div :key="active" class="grid">
                <section v-for="[group, items] in groups" :id="slug(group)" :key="group" class="card" :style="{ '--tone': meta(group).tone }">
                  <div class="card-head">
                    <span class="badge"><Icon :name="meta(group).icon" /></span>
                    <div>
                      <h2>{{ group }}</h2>
                      <p v-if="meta(group).sub">{{ meta(group).sub }}</p>
                    </div>
                  </div>
                  <div class="card-body">
                    <SettingRow v-for="s in items" :key="s.key" :setting="s" :disabled="busy" @change="change(s, $event)" />
                  </div>
                </section>
              </div>

              <section v-if="dev && unsupported.length" class="card unsupported" :style="{ '--tone': '#64748b' }">
                <div class="card-head">
                  <span class="badge"><Icon name="bug" /></span>
                  <div>
                    <h2>Unsupported on this monitor</h2>
                    <p>Dev build only · {{ unsupported.length }} setting{{ unsupported.length === 1 ? "" : "s" }} hidden</p>
                  </div>
                </div>
                <div class="card-body">
                  <div v-for="u in unsupported" :key="u.key + u.code" class="row disabled">
                    <label>{{ u.label }} <code>0x{{ u.code.toString(16).toUpperCase().padStart(2, "0") }}</code></label>
                    <span class="muted">{{ u.reason }}</span>
                  </div>
                </div>
              </section>
            </template>
          </template>

          <div v-else class="grid">
            <section :id="slug('Settings')" class="card" :style="{ '--tone': meta('Settings').tone }">
              <div class="card-head">
                <span class="badge"><Icon :name="meta('Settings').icon" /></span>
                <div>
                  <h2>Settings</h2>
                  <p>{{ meta("Settings").sub }}</p>
                </div>
              </div>
              <div class="card-body">
                <div class="row" title="Starts AsusDisplay when you log in (XDG autostart entry).">
                  <label for="autostart">Launch at login</label>
                  <input
                    id="autostart"
                    type="checkbox"
                    role="switch"
                    :checked="autostart"
                    :disabled="busy"
                    @change="toggleAutostart(($event.target as HTMLInputElement).checked)"
                  />
                </div>
              </div>
            </section>

            <section :id="slug('About')" class="card" :style="{ '--tone': meta('About').tone }">
              <div class="card-head">
                <span class="badge"><Icon :name="meta('About').icon" /></span>
                <div>
                  <h2>About</h2>
                  <p>{{ meta("About").sub }}</p>
                </div>
              </div>
              <div class="card-body">
                <div class="row">
                  <label>Application</label>
                  <span class="info">{{ about?.name ?? "AsusDisplay" }}</span>
                </div>
                <div class="row">
                  <label>Version</label>
                  <span class="info">{{ about?.version ?? "…" }}</span>
                </div>
                <div class="row">
                  <label>Build</label>
                  <span class="info">{{ dev ? "Development" : "Release" }}</span>
                </div>
                <div class="row">
                  <label>Tauri</label>
                  <span class="info">{{ about?.tauri ?? "…" }}</span>
                </div>
                <div class="row">
                  <label>License</label>
                  <span class="info">MIT</span>
                </div>
                <div class="row">
                  <label>Source</label>
                  <span class="info selectable">{{ REPOSITORY }}</span>
                </div>
              </div>
            </section>
          </div>
        </div>
      </main>
    </div>
  </div>

  <dialog ref="dialog" @cancel.prevent="answer(false)">
    <span class="tile"><Icon name="alert" /></span>
    <h3>{{ pending?.label }}?</h3>
    <p>{{ pending?.note }}</p>
    <footer>
      <button class="btn ghost" @click="answer(false)">Cancel</button>
      <button class="btn primary" @click="answer(true)">Continue</button>
    </footer>
  </dialog>
</template>
