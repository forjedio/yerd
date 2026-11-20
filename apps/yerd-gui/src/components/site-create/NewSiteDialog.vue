<script setup lang="ts">
import {
  ArrowLeft,
  ArrowRight,
  CircleCheck,
  Code2,
  ExternalLink,
  FolderOpen,
  Loader2,
  Sparkles,
  UserRound,
} from "lucide-vue-next";
import { computed, onUnmounted, reactive, ref, watch } from "vue";

import CreateProgress from "@/components/site-create/CreateProgress.vue";
import LaravelOptionsStep from "@/components/site-create/LaravelOptionsStep.vue";
import MissingToolsPanel from "@/components/site-create/MissingToolsPanel.vue";
import ProjectStep from "@/components/site-create/ProjectStep.vue";
import WordPressOptionsStep from "@/components/site-create/WordPressOptionsStep.vue";
import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useCreateSiteJob } from "@/composables/useCreateSiteJob";
import { useLaravelStarter } from "@/composables/useLaravelStarter";
import { usePlatform } from "@/composables/usePlatform";
import { type SiteBasicsForm, useSiteBasics } from "@/composables/useSiteBasics";
import { openSiteInEditor } from "@/composables/useSiteEditor";
import {
  type ActiveStarter,
  type StarterContext,
  type StarterId,
  FRAMEWORKS,
  useComposerStarter,
} from "@/composables/useSiteStarters";
import { useToolPrereqs } from "@/composables/useToolPrereqs";
import { useWordPressStarter } from "@/composables/useWordPressStarter";
import { openInBrowser, openPath } from "@/ipc/client";
import type { ComposerFramework, StatusReport } from "@/ipc/types";
import { formatElapsed } from "@/lib/createSite";
import { siteNameTaken } from "@/lib/sites";
import { cn } from "@/lib/utils";

/**
 * The New site dialog: Project (starter, name, location, PHP, HTTPS), then the
 * missing-tools step when the starter's toolchain is incomplete, then the
 * starter's Options, then the create job's progress and Done screen. Switching
 * starters keeps what was typed on Project. There is one create at a time: it
 * keeps running when the dialog closes or unmounts, and reopening shows its
 * progress. A name an existing site already uses (linked or parked, any case)
 * is refused, since creating it would reconfigure that site instead.
 */
const props = defineProps<{
  open: boolean;
  parkedFolders: string[];
  phpVersions: string[];
  defaultPhp: string;
  tld: string;
  /** Live daemon status, for URLs that match the rest of the GUI. */
  report: StatusReport | null;
  home: string | null;
  initialStarter?: StarterId;
  initialName?: string;
  /** Every existing site's name, so a taken name is refused before the job starts. */
  existingNames?: string[];
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  created: [];
  existing: [];
}>();

const { supportsPathInstall, isMac } = usePlatform();

const basics = reactive<SiteBasicsForm>({ name: "", location: "", php: "", secure: false });
const { siteName, nameValid, projectPath, domain, openUrl, basicsValid } = useSiteBasics(
  basics,
  props,
);
const nameTaken = computed(() => siteNameTaken(basics.name, props.existingNames ?? []));

const prereqs = reactive(useToolPrereqs(() => props.open));
const ctx: StarterContext = {
  phpVersions: () => props.phpVersions,
  defaultPhp: () => props.defaultPhp,
  prereqs,
  report: () => props.report,
};

const laravel = reactive(useLaravelStarter(basics, ctx));
const wordpress = reactive(useWordPressStarter(basics, ctx));
const composer = (framework: ComposerFramework) =>
  reactive(useComposerStarter(framework, basics, ctx));
const composerModels = {
  codeigniter: composer("codeigniter"),
  cakephp: composer("cakephp"),
  slim: composer("slim"),
};
const models: Record<StarterId, ActiveStarter> = {
  laravel,
  wordpress,
  ...composerModels,
};

const starter = ref<StarterId>("laravel");
const active = computed<ActiveStarter>(() => models[starter.value]);
const composerPkg = computed(() =>
  starter.value in FRAMEWORKS ? FRAMEWORKS[starter.value as ComposerFramework].pkg : "",
);

type Step = "project" | "tools" | "options" | "progress";
const step = ref<Step>("project");

const job = useCreateSiteJob({
  isOpen: () => props.open,
  onCreated: () => emit("created"),
});
const { state, phase, log, error, cancelRequested, inFlight, startedAt, finishedAt, snapshot } = job;

/**
 * Keep the PHP pick valid: fill it when empty, and replace it when the starter
 * doesn't support it or it is no longer installed. A valid pick (a legacy one
 * for WordPress included) is never overwritten by a status tick, and a PHP the
 * tools step installs fills an empty field.
 */
function reconcilePhp(): void {
  const php = basics.php;
  if (!php || !active.value.supportsPhp(php) || !props.phpVersions.includes(php)) {
    basics.php = active.value.preferredPhp();
  }
}

watch([() => props.phpVersions, starter], reconcilePhp);

function selectStarter(id: StarterId): void {
  if (id === starter.value) return;
  starter.value = id;
  active.value.reset();
  reconcilePhp();
}

function resetAll(): void {
  basics.name = props.initialName ?? "";
  basics.location = props.parkedFolders[0] ?? "";
  basics.php = "";
  basics.secure = false;
  starter.value = props.initialStarter ?? "laravel";
  step.value = "project";
  job.reset();
  active.value.reset();
  prereqs.reset();
  void prereqs.refresh();
  reconcilePhp();
}

/** Put back the form a running create was started from, for a dialog that
 *  was remounted (the Sites view was left) while the job carried on. */
function restoreFromSnapshot(): void {
  const snap = snapshot.value;
  if (!snap) return;
  if (snap.starter !== starter.value) {
    starter.value = snap.starter;
    active.value.reset();
  }
  basics.name = snap.name;
  basics.location = snap.location;
  basics.php = snap.php;
  basics.secure = snap.secure;
}

/** Reopening shows a running create's progress, and leaves a tools step that
 *  is still installing as it was, so its install isn't forgotten and re-run. */
watch(
  () => props.open,
  (open) => {
    if (!open) return;
    if (inFlight.value) {
      restoreFromSnapshot();
      step.value = "progress";
    } else if (!prereqs.busy) {
      resetAll();
    }
  },
  { immediate: true },
);

watch(
  () => active.value.ready,
  (ready) => {
    if (ready && step.value === "tools") step.value = "options";
  },
);

const canNext = computed(
  () => !prereqs.loading && nameValid.value && !nameTaken.value && basics.location.trim() !== "",
);

function next(): void {
  if (!canNext.value) return;
  step.value = active.value.ready ? "options" : "tools";
}

const canCreate = computed(
  () => active.value.ready && basicsValid.value && active.value.optionsValid,
);

function create(): void {
  if (!canCreate.value || inFlight.value) return;
  if (nameTaken.value) {
    step.value = "project";
    return;
  }
  step.value = "progress";
  void job.start(active.value.buildSpec(), {
    starter: starter.value,
    name: basics.name,
    location: basics.location,
    php: basics.php,
    secure: basics.secure,
    domain: domain.value,
    doneSummary: active.value.doneSummary,
  });
}

function close(): void {
  emit("update:open", false);
}

function existing(): void {
  emit("existing");
  close();
}

const running = computed(() => step.value === "progress" && state.value === "running");

const now = ref(Date.now());
let clock: number | null = null;

function stopClock(): void {
  if (clock !== null) {
    window.clearInterval(clock);
    clock = null;
  }
}

watch(
  running,
  (on) => {
    stopClock();
    if (!on) return;
    now.value = Date.now();
    clock = window.setInterval(() => {
      now.value = Date.now();
    }, 1000);
  },
  { immediate: true },
);

onUnmounted(stopClock);

const elapsed = computed(() =>
  startedAt.value === null ? "" : formatElapsed((finishedAt.value ?? now.value) - startedAt.value),
);

const title = computed(() =>
  step.value === "options" || step.value === "progress" ? `New ${active.value.label} site` : "New site",
);

const STEPS = [
  { id: 1, label: "Project" },
  { id: 2, label: "Options" },
];

const currentStepNumber = computed(() => (step.value === "options" ? 2 : 1));
</script>

<template>
  <DialogFrame :open="open" :title="title" @update:open="emit('update:open', $event)">
    <template v-if="running" #title>
      <Loader2 class="size-4 shrink-0 animate-spin text-brand" aria-hidden="true" />
      <span class="truncate">Creating {{ domain }}</span>
    </template>

    <template #header-extra>
      <span v-if="step === 'progress' && elapsed" class="font-mono text-xs text-faint" data-testid="ns-elapsed">
        {{ elapsed }}
      </span>
      <ol v-else-if="step !== 'progress'" class="flex items-center gap-2" aria-label="Steps">
        <template v-for="(s, i) in STEPS" :key="s.id">
          <li v-if="i > 0" class="h-px w-5 bg-border" aria-hidden="true" />
          <li
            :class="
              cn(
                'flex h-6 items-center gap-1.5 rounded-full px-2 text-xs',
                s.id === currentStepNumber
                  ? 'border border-brand/40 bg-brand/15 font-semibold text-foreground'
                  : 'text-muted-foreground',
              )
            "
            :aria-current="s.id === currentStepNumber ? 'step' : undefined"
          >
            <CircleCheck v-if="s.id < currentStepNumber" class="size-4 text-success" aria-hidden="true" />
            <span
              v-else
              :class="
                cn(
                  'flex size-4 items-center justify-center rounded-full text-[10px] font-semibold',
                  s.id === currentStepNumber ? 'bg-brand text-white' : 'ring-1 ring-border',
                )
              "
            >
              {{ s.id }}
            </span>
            {{ s.label }}
          </li>
        </template>
      </ol>
    </template>

    <ProjectStep
      v-if="step === 'project'"
      :starter="starter"
      v-model:name="basics.name"
      v-model:location="basics.location"
      v-model:php="basics.php"
      v-model:secure="basics.secure"
      :model="active"
      :parked-folders="parkedFolders"
      :php-versions="phpVersions"
      :tld="tld"
      :home="home"
      :open-url="openUrl"
      :project-path="projectPath"
      :name-valid="nameValid"
      :name-taken="nameTaken"
      :ca-trusted="report?.ca.trusted_system === true"
      @update:starter="selectStarter"
      @existing="existing"
      @submit="next"
    />

    <MissingToolsPanel v-else-if="step === 'tools'" :model="active" :prereqs="prereqs" />

    <template v-else-if="step === 'options'">
      <LaravelOptionsStep v-if="starter === 'laravel'" :starter="laravel" />
      <WordPressOptionsStep v-else-if="starter === 'wordpress'" :starter="wordpress" />
      <p v-else class="text-[13px] text-muted-foreground" data-testid="ns-composer-note">
        Scaffolds <code class="font-mono text-foreground">{{ composerPkg }}</code> with
        <code class="font-mono">composer create-project</code>.
      </p>
    </template>

    <CreateProgress
      v-else
      :phases="active.phases"
      :phase="phase"
      :state="state"
      :log="log"
      :error="error"
      :domain="domain"
      :open-url="openUrl"
      :secure="basics.secure"
      :done-summary="snapshot?.doneSummary ?? active.doneSummary"
      :started-at="startedAt"
      :finished-at="finishedAt"
    />

    <template #footer-start>
      <template v-if="step === 'project'">Return to continue</template>
      <Button v-else-if="step === 'tools' || step === 'options'" variant="ghost" size="sm" @click="step = 'project'">
        <ArrowLeft /> Back
      </Button>
      <template v-else-if="running">You can close this — it keeps going in the background</template>
      <Button
        v-else-if="state === 'failed' || state === 'cancelled'"
        variant="ghost"
        size="sm"
        @click="step = 'options'"
      >
        <ArrowLeft /> Back
      </Button>
    </template>

    <template #footer>
      <template v-if="step === 'project'">
        <Button variant="outline" @click="close">Cancel</Button>
        <Button :disabled="!canNext" @click="next">
          <Spinner v-if="prereqs.loading" class="size-4" />
          Next: Options <ArrowRight />
        </Button>
      </template>

      <template v-else-if="step === 'tools'">
        <Button variant="outline" @click="close">Cancel</Button>
        <Button :disabled="prereqs.busy" @click="active.installAll()">
          <Spinner v-if="prereqs.installingAll" class="size-4" />
          {{ prereqs.installingAll ? "Installing…" : "Install missing tools" }}
        </Button>
      </template>

      <template v-else-if="step === 'options'">
        <Button variant="outline" @click="close">Cancel</Button>
        <Button :disabled="!canCreate" @click="create"><Sparkles /> Create site</Button>
      </template>

      <template v-else-if="running">
        <Button variant="outline" :disabled="cancelRequested" @click="job.cancel()">
          {{ cancelRequested ? "Cancelling…" : "Cancel" }}
        </Button>
      </template>

      <template v-else-if="state === 'succeeded'">
        <Button v-if="supportsPathInstall" variant="outline" @click="openSiteInEditor(siteName)">
          <Code2 /> Open in editor
        </Button>
        <Button variant="outline" @click="openPath(projectPath)">
          <FolderOpen /> {{ isMac ? "Finder" : "Files" }}
        </Button>
        <Button
          v-if="starter === 'wordpress'"
          variant="outline"
          title="Signs you in as the site's admin when possible"
          @click="wordpress.openWpAdmin()"
        >
          <UserRound /> WP Admin
        </Button>
        <Button @click="openInBrowser(openUrl)"><ExternalLink /> Open site</Button>
      </template>

      <template v-else>
        <Button variant="outline" @click="close">Close</Button>
      </template>
    </template>
  </DialogFrame>
</template>
