<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";

import { cn } from "@/lib/utils";

/**
 * The chrome around a list screen's inspector. In `panel` mode it is a plain
 * scrolling column beside the list (wide windows); in `drawer` mode it slides
 * over the window from the right with a backdrop and closes on Escape or a
 * backdrop click. Escape is left alone while `layerOpen` returns true or while
 * another modal layer sits above the drawer. The content goes in the default
 * slot.
 */
const props = defineProps<{
  mode: "panel" | "drawer";
  /** Whether there is anything to show (and, in drawer mode, whether it is open). */
  shown: boolean;
  /** The aside's accessible name. */
  label: string;
  /** The aside's `data-testid`. */
  testid: string;
  /** An extra class on the drawer backdrop, for callers' selectors. */
  backdropClass?: string;
  /** Whether a menu or popover inside the content owns Escape right now. A
   *  getter rather than a boolean, so opening a menu doesn't re-render the frame. */
  layerOpen?: () => boolean;
}>();

const emit = defineEmits<{ close: [] }>();

const drawer = computed(() => props.mode === "drawer");
const panel = ref<HTMLElement | null>(null);

/** Whether another modal layer sits above the drawer. Dialogs opened from inside
 *  it and the owning view's confirms teleport into `<body>` after the drawer, so
 *  a later dialog in document order sits on top and owns Escape: without this
 *  both listeners fire and one keypress dismisses the pair. A drawer that isn't
 *  in the document (a detached test mount) has index -1, so any other open
 *  dialog counts as being above it. */
function hasDialogAbove(): boolean {
  const dialogs = Array.from(
    document.querySelectorAll('[role="dialog"][aria-modal="true"], [role="alertdialog"]'),
  );
  const own = panel.value ? dialogs.indexOf(panel.value) : -1;
  return dialogs.some((dialog, index) => dialog !== panel.value && index > own);
}

function onKeydown(event: KeyboardEvent): void {
  if (!drawer.value || !props.shown || event.key !== "Escape") return;
  if (props.layerOpen?.() || hasDialogAbove()) return;
  emit("close");
}

onMounted(() => document.addEventListener("keydown", onKeydown));
onUnmounted(() => document.removeEventListener("keydown", onKeydown));
</script>

<template>
  <Teleport to="body" :disabled="!drawer">
    <Transition name="inspector-backdrop">
      <div
        v-if="drawer && shown"
        :class="
          cn(
            'fixed inset-0 z-40 rounded-[10px] bg-black/50 [html.window-maximized_&]:rounded-none',
            backdropClass,
          )
        "
        aria-hidden="true"
        @click="emit('close')"
      />
    </Transition>

    <!-- In drawer mode a teleported overlay escapes the window's rounded clip, so
         this wrapper restores it over the full window height. It stays mounted,
         paint-free, for the leave transition. -->
    <div
      :class="
        drawer
          ? 'pointer-events-none fixed inset-0 z-50 overflow-hidden rounded-[10px] [html.window-maximized_&]:rounded-none'
          : 'h-full'
      "
    >
      <Transition :name="drawer ? 'inspector-panel' : 'inspector-none'">
        <aside
          v-if="shown"
          ref="panel"
          :class="
            cn(
              'flex flex-col overflow-y-auto bg-background',
              drawer
                ? 'pointer-events-auto absolute inset-y-0 right-0 w-[628px] max-w-full border-l shadow-2xl'
                : 'h-full',
            )
          "
          :aria-label="label"
          :role="drawer ? 'dialog' : undefined"
          :aria-modal="drawer ? 'true' : undefined"
          :data-testid="testid"
          @click.stop
        >
          <slot />
        </aside>
      </Transition>
    </div>
  </Teleport>
</template>

<style scoped>
.inspector-backdrop-enter-active,
.inspector-backdrop-leave-active {
  transition: opacity 180ms ease;
}

.inspector-backdrop-enter-from,
.inspector-backdrop-leave-to {
  opacity: 0;
}

.inspector-panel-enter-active,
.inspector-panel-leave-active {
  transition: transform 220ms ease;
}

.inspector-panel-enter-from,
.inspector-panel-leave-to {
  transform: translateX(100%);
}
</style>
