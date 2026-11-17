import * as clipboard from "./features/clipboard";
import * as notes from "./features/notes";
import * as screenshot from "./features/screenshot";

// Each feature owns its folder and exports mount(el). A tab mounts its feature the first time it is shown.
const features: Record<string, { mount(el: HTMLElement): void }> = { clipboard, screenshot, notes };
const mounted = new Set<string>();

function show(name: string): void {
  for (const tab of document.querySelectorAll<HTMLButtonElement>("[data-tab]")) {
    const active = tab.dataset.tab === name;
    tab.setAttribute("aria-selected", String(active));
    document.getElementById(`panel-${tab.dataset.tab}`)!.hidden = !active;
  }
  if (!mounted.has(name)) {
    mounted.add(name);
    features[name].mount(document.getElementById(`panel-${name}`)!);
  }
}

for (const tab of document.querySelectorAll<HTMLButtonElement>("[data-tab]")) {
  tab.addEventListener("click", () => show(tab.dataset.tab!));
}
show("clipboard");
