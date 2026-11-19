<script setup lang="ts">
import { CircleCheck, Rocket, RotateCcw } from "lucide-vue-next";
import { computed, ref } from "vue";
import { RouterLink } from "vue-router";

import Button, { buttonVariants } from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useOnboarding } from "@/composables/useOnboarding";
import { invalidate } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import { IpcError, park, pickDirectory } from "@/ipc/client";
import type { StatusReport } from "@/ipc/types";
import { nextStep, setupSteps, type SetupStepKey } from "@/lib/overview";
import { cn } from "@/lib/utils";

/**
 * The first-run checklist shown while the environment is empty. Each step hands
 * off to the page that already owns it (Doctor for trust, PHP for installs) or
 * runs the same park flow as the Sites page.
 */
const props = defineProps<{ report: StatusReport; parkedRoots: number }>();

const { relaunch } = useOnboarding();
const { refresh } = useDaemon();
const toast = useToast();

interface StepCopy {
  title: string;
  subtitle: string;
  action: string;
  to?: string;
}

const COPY = computed<Record<SetupStepKey, StepCopy>>(() => ({
  trust: {
    title: "Trust the local certificate authority",
    subtitle: "Lets your browser open sites over HTTPS without warnings",
    action: "Trust",
    to: "/doctor",
  },
  php: {
    title: "Install a PHP version",
    subtitle: "Pick a version on the PHP page, you can add more later",
    action: "Install",
    to: "/php",
  },
  park: {
    title: "Park your projects folder",
    subtitle: `Every folder inside becomes a .${props.report.tld} site`,
    action: "Choose…",
  },
}));

const steps = computed(() => setupSteps(props.report, props.parkedRoots));
const next = computed(() => nextStep(steps.value));
const parking = ref(false);

async function onPark(): Promise<void> {
  const dir = await pickDirectory();
  if (!dir) return;
  parking.value = true;
  try {
    await park(dir);
    toast.success("Parked directory", dir);
    await invalidate("sites");
    await refresh();
  } catch (e) {
    toast.error("Park failed", (e as IpcError).message);
  } finally {
    parking.value = false;
  }
}
</script>

<template>
  <section
    data-testid="setup-checklist"
    class="rounded-xl border border-brand/40 bg-card px-5 pb-4 pt-[18px] text-card-foreground"
  >
    <div class="flex items-center gap-2">
      <Rocket class="size-4 text-brand" aria-hidden="true" />
      <h3 class="text-[15px] font-semibold">Let's get you set up</h3>
    </div>
    <p class="mt-2 text-[13px] text-muted-foreground">
      Three quick steps and your projects are live on
      <span class="font-mono">.{{ report.tld }}</span>.
    </p>

    <ol class="mt-3 overflow-hidden rounded-lg border bg-muted dark:bg-background">
      <li
        v-for="(step, i) in steps"
        :key="step.key"
        :data-step="step.key"
        :class="
          cn(
            'flex items-center gap-3 px-3.5 py-3',
            i > 0 && 'border-t',
            step.key === next && 'bg-brand/5',
          )
        "
      >
        <CircleCheck v-if="step.done" class="size-4 shrink-0 text-success" aria-hidden="true" />
        <span
          v-else
          :class="
            cn(
              'flex size-4 shrink-0 items-center justify-center rounded-full text-[10px] font-semibold',
              step.key === next
                ? 'bg-brand text-brand-foreground'
                : 'border border-muted-foreground text-muted-foreground',
            )
          "
          aria-hidden="true"
        >
          {{ i + 1 }}
        </span>
        <div class="min-w-0 flex-1">
          <p
            :class="
              cn(
                'truncate text-[13px] font-medium',
                step.done && 'text-muted-foreground line-through',
              )
            "
          >
            {{ COPY[step.key].title }}
          </p>
          <p class="truncate text-xs text-faint">
            {{ step.done ? "Done" : COPY[step.key].subtitle }}
          </p>
        </div>
        <template v-if="!step.done">
          <RouterLink
            v-if="COPY[step.key].to"
            :to="COPY[step.key].to ?? '/'"
            :class="
              cn(
                buttonVariants({ variant: step.key === next ? 'default' : 'outline', size: 'sm' }),
                'shrink-0',
              )
            "
          >
            {{ COPY[step.key].action }}
          </RouterLink>
          <Button
            v-else
            :variant="step.key === next ? 'default' : 'outline'"
            size="sm"
            class="shrink-0"
            :disabled="parking"
            @click="onPark"
          >
            <Spinner v-if="parking" class="!size-3.5 text-current" />
            {{ COPY[step.key].action }}
          </Button>
        </template>
      </li>
    </ol>

    <button
      type="button"
      class="mt-3 inline-flex items-center gap-1.5 rounded-md text-[13px] font-medium text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      @click="relaunch"
    >
      <RotateCcw class="size-3.5" aria-hidden="true" /> Re-run the full guided setup
    </button>
  </section>
</template>
