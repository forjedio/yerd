<script setup lang="ts">
import { Copy } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import InfoBanner from "@/components/ui/InfoBanner.vue";
import { useToast } from "@/composables/useToast";

/**
 * The registration snippets for Yerd's MCP server, opened from the AI agents
 * row while it is on. Registering points the agent at the `yerd` binary by
 * name, so when that can't resolve yet (`needsCliPath`, macOS without the
 * Terminal CLI shim) it shows only the note to install it first.
 */
defineProps<{ open: boolean; needsCliPath: boolean }>();

const emit = defineEmits<{ "update:open": [boolean] }>();

const toast = useToast();

const MCP_CLAUDE_SNIPPET = "claude mcp add --scope user yerd -- yerd mcp";
const MCP_JSON_SNIPPET = JSON.stringify(
  { mcpServers: { yerd: { command: "yerd", args: ["mcp"] } } },
  null,
  2,
);

async function copyMcpSnippet(snippet: string, label: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(snippet);
    toast.success(`${label} copied`);
  } catch {
    toast.error("Couldn't copy to the clipboard");
  }
}
</script>

<template>
  <DialogFrame
    :open="open"
    title="Connect an agent"
    width="lg"
    @update:open="emit('update:open', $event)"
  >
    <p class="text-[13px] text-muted-foreground">
      Yerd's MCP server is on. Register it once with each agent you use.
    </p>

    <InfoBanner v-if="needsCliPath" class="mt-4" data-testid="mcp-needs-cli">
      Install <code>yerd</code> on your PATH first (Terminal CLI, under Integrations and
      security), then come back here for the registration command.
    </InfoBanner>

    <template v-else>
      <p class="mt-4 text-[13.5px] font-semibold">Claude Code</p>
      <p class="mt-0.5 text-[12.5px] text-muted-foreground">Run this in a terminal.</p>
      <div class="relative mt-2 rounded-lg border bg-sidebar p-3.5 pr-12 font-mono text-xs">
        <code data-testid="mcp-command" class="break-all">{{ MCP_CLAUDE_SNIPPET }}</code>
        <Button
          variant="outline"
          size="icon"
          class="absolute right-2 top-2 size-7"
          aria-label="Copy command"
          @click="copyMcpSnippet(MCP_CLAUDE_SNIPPET, 'Command')"
        >
          <Copy />
        </Button>
      </div>

      <p class="mt-4 text-[13.5px] font-semibold">Other agents</p>
      <p class="mt-0.5 text-[12.5px] text-muted-foreground">Add this to the agent's MCP config.</p>
      <div class="relative mt-2 rounded-lg border bg-sidebar p-3.5 pr-12">
        <pre
          data-testid="mcp-config"
          class="overflow-x-auto font-mono text-xs"
        ><code>{{ MCP_JSON_SNIPPET }}</code></pre>
        <Button
          variant="outline"
          size="icon"
          class="absolute right-2 top-2 size-7"
          aria-label="Copy config"
          @click="copyMcpSnippet(MCP_JSON_SNIPPET, 'Config')"
        >
          <Copy />
        </Button>
      </div>
    </template>

    <template #footer>
      <Button @click="emit('update:open', false)">Done</Button>
    </template>
  </DialogFrame>
</template>
