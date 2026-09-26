<script setup lang="ts">
import { computed, reactive, watch } from "vue";

import CreateSiteWizardShell from "@/components/site-create/CreateSiteWizardShell.vue";
import SiteBasicsStep from "@/components/site-create/SiteBasicsStep.vue";
import { useSiteBasics } from "@/composables/useSiteBasics";
import { type PrereqRow, useToolPrereqs } from "@/composables/useToolPrereqs";
import { comparePhpVersions } from "@/lib/phpVersion";
import type { ComposerFramework, CreateSiteSpec, StatusReport } from "@/ipc/types";

const props = defineProps<{
  open: boolean;
  framework: ComposerFramework;
  parkedFolders: string[];
  phpVersions: string[];
  defaultPhp: string;
  tld: string;
  report: StatusReport | null;
}>();
const emit = defineEmits<{
  (e: "update:open", v: boolean): void;
  (e: "created"): void;
}>();

const FRAMEWORKS: Record<ComposerFramework, { label: string; pkg: string; minPhp: string }> = {
  codeigniter: { label: "CodeIgniter", pkg: "codeigniter4/appstarter", minPhp: "8.1" },
  cakephp: { label: "CakePHP", pkg: "cakephp/app", minPhp: "8.1" },
  slim: { label: "Slim", pkg: "slim/skeleton", minPhp: "7.4" },
};
const meta = computed(() => FRAMEWORKS[props.framework]);

const PHASES = ["Preflight", "Scaffolding", "Registering", "Done"];

const form = reactive({ name: "", location: "", php: "", secure: false });
const { nameValid, projectPath, domain, openUrl, basicsValid } = useSiteBasics(form, props);

function supportsPhp(v: string): boolean {
  return comparePhpVersions(v, meta.value.minPhp) >= 0;
}
const supportedPhpVersions = computed(() => props.phpVersions.filter(supportsPhp));
const phpOptions = computed(() =>
  supportedPhpVersions.value.map((v) => ({ value: v, label: `PHP ${v}` })),
);

function preferredPhp(): string {
  const supported = supportedPhpVersions.value;
  if (props.defaultPhp && supported.includes(props.defaultPhp)) return props.defaultPhp;
  return supported[supported.length - 1] ?? "";
}

const prereqs = reactive(useToolPrereqs(() => props.open));
const noSupportedPhp = computed(() => supportedPhpVersions.value.length === 0);
const needsComposer = computed(() => !prereqs.available("composer"));
const ready = computed(() => !noSupportedPhp.value && !needsComposer.value);

function installPhp(): Promise<boolean> {
  return prereqs.installPhp(
    supportsPhp,
    `No installable PHP ${meta.value.minPhp} or newer was found.`,
  );
}

const prereqRows = computed<PrereqRow[]>(() => [
  {
    id: "php",
    label: "PHP",
    sub:
      noSupportedPhp.value && props.phpVersions.length > 0
        ? `Installed, but not ${meta.value.minPhp}+`
        : `Runtime ${meta.value.minPhp}+`,
    ok: !noSupportedPhp.value,
    install: installPhp,
  },
  {
    id: "composer",
    label: "Composer",
    sub: "Dependency manager",
    ok: !needsComposer.value,
    install: () => prereqs.installTool("composer"),
  },
]);

function installAll(): Promise<void> {
  return prereqs.installAll(async () => {
    if (noSupportedPhp.value && !(await installPhp())) return false;
    if (needsComposer.value && !(await prereqs.installTool("composer"))) return false;
    return true;
  });
}

function buildSpec(): CreateSiteSpec {
  return {
    name: form.name.trim(),
    parent_dir: form.location,
    php: form.php,
    secure: form.secure,
    framework: { framework: props.framework },
  };
}

function reset(): void {
  form.name = "";
  form.location = props.parkedFolders[0] ?? "";
  form.php = preferredPhp();
  form.secure = false;
  prereqs.reset();
  void prereqs.refresh();
}

watch(
  () => props.phpVersions,
  () => {
    if (!form.php || !supportedPhpVersions.value.includes(form.php)) {
      form.php = preferredPhp();
    }
  },
);
</script>

<template>
  <CreateSiteWizardShell
    :open="open"
    :title="`Create a new ${meta.label} site`"
    :steps="['Basics', 'Review']"
    :step-valid="[basicsValid, true]"
    :build-spec="buildSpec"
    :phases="PHASES"
    :domain="domain"
    :project-path="projectPath"
    :open-url="openUrl"
    :prereqs="prereqs"
    :prereq-rows="prereqRows"
    :prereq-summary="`Creating a ${meta.label} site needs PHP and Composer.`"
    :ready="ready"
    :install-all="installAll"
    @update:open="(v) => emit('update:open', v)"
    @created="emit('created')"
    @reset="reset"
  >
    <template #step-0>
      <SiteBasicsStep
        v-model:name="form.name"
        v-model:location="form.location"
        v-model:php="form.php"
        v-model:secure="form.secure"
        id-prefix="ccs"
        :parked-folders="parkedFolders"
        :php-options="phpOptions"
        :domain="domain"
        :project-path="projectPath"
        :name-valid="nameValid"
        no-php-text="No supported PHP installed."
      >
        <template #php-hint>
          The version this site runs on. {{ meta.label }} needs PHP {{ meta.minPhp }} or newer.
        </template>
      </SiteBasicsStep>
    </template>

    <template #step-1>
      <div class="space-y-4">
        <div class="rounded-lg border p-3 text-sm">
          <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5">
            <dt class="text-muted-foreground">Site</dt>
            <dd class="font-mono">{{ domain }}</dd>
            <dt class="text-muted-foreground">Path</dt>
            <dd class="truncate font-mono">{{ projectPath }}</dd>
            <dt class="text-muted-foreground">PHP</dt>
            <dd>{{ form.php }}{{ form.secure ? " · HTTPS" : "" }}</dd>
            <dt class="text-muted-foreground">Template</dt>
            <dd class="font-mono">{{ meta.pkg }}</dd>
          </dl>
        </div>
      </div>
    </template>
  </CreateSiteWizardShell>
</template>
