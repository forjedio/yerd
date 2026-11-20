<script setup lang="ts">
import { ArrowRight, FileDown, ImageOff, Paperclip, Trash2, X } from "lucide-vue-next";
import { TabsContent, TabsList, TabsRoot, TabsTrigger } from "reka-ui";
import { computed, nextTick, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useToast } from "@/composables/useToast";
import { IpcError, openInBrowser, openInEditor, saveMailAttachment } from "@/ipc/client";
import type { MailAttachment, MailDetail } from "@/ipc/types";
import {
  attachmentsLabel,
  mimeLabel,
  readerTime,
  remoteKindLabel,
  remoteSummary,
  senderParts,
} from "@/lib/mail";
import {
  buildMailFrameDocument,
  findFragmentTarget,
  linkifyText,
  listRemoteContentUrls,
  resolveExternalHref,
  resolveFrameLink,
} from "@/lib/mailLinks";
import { humaniseBytes } from "@/lib/utils";

/**
 * One captured message: subject, sender and recipients, Delete, the
 * attachments band and Preview / Text / Headers tabs. Shared by the Mail
 * screen (panel or drawer) and the Mails window.
 *
 * HTML mail is sanitized with DOMPurify, then rendered in a host Shadow DOM so
 * link clicks are handled in-process (WKWebView does not reliably deliver
 * sandboxed iframe clicks / postMessage). Opens are scheme-allowlisted. Remote
 * images and stylesheets stay blocked until the user loads them, per message.
 * Every link click is cancelled: the app uses a hash router, so an in-mail `#`
 * link would otherwise navigate the app; `#fragment` links scroll the reader.
 *
 * The shadow host sits inside a frame that contains the message: `contain:
 * layout paint` and a transform make the frame the containing block for
 * `position: fixed` mail content and clip it, and `isolation` keeps the mail's
 * z-indexes inside. Mail CSS cannot select the frame (`:host` only reaches the
 * shadow host, and those rules are disabled anyway), so a message cannot lay
 * itself over the app chrome.
 */
const props = defineProps<{
  detail: MailDetail | null;
  loading: boolean;
  /** Epoch seconds, for the "Today 14:32" style time. */
  now: number;
  deleting: boolean;
  /** Show a close button beside Delete (drawer mode). */
  closable?: boolean;
  /** Yerd's site TLD: images and styles on it (and on localhost) load without opting in. */
  tld?: string;
  /** The selected message whose body could not be read; the reader still offers Delete for it. */
  failedId?: string | null;
}>();

const emit = defineEmits<{ delete: [id: string]; close: [] }>();

type ReaderTab = "preview" | "text" | "headers";

const toast = useToast();

const tab = ref<ReaderTab>("preview");
/** Opt-in: remote images / stylesheets phone home; off until the user asks.
 *  Local ones (on `tld`, localhost or loopback) always load. */
const loadRemote = ref(false);
const confirmOpen = ref(false);
const pendingDelete = ref<{ id: string; subject: string } | null>(null);

watch(
  () => props.detail,
  (d) => {
    tab.value = d?.html_body ? "preview" : "text";
    loadRemote.value = false;
  },
  { immediate: true },
);

const sender = computed(() => senderParts(props.detail?.from ?? ""));
const subject = computed(() => props.detail?.subject?.trim() || "(no subject)");
const attachments = computed<MailAttachment[]>(() => props.detail?.attachments ?? []);

const remoteContent = computed(() => {
  const html = props.detail?.html_body;
  return html ? listRemoteContentUrls(html, props.tld) : [];
});

const remoteLabel = computed(() => remoteSummary(remoteContent.value, loadRemote.value));

/** Linkified plain-text body. Computed (not called inline in the template) so
 *  it only re-runs when the body changes, not on every 4s poll re-render. */
const linkedTextBody = computed(() => linkifyText(props.detail?.text_body || "(empty message)"));

function askDelete(): void {
  const d = props.detail;
  const id = d?.id ?? props.failedId;
  if (!id) return;
  pendingDelete.value = { id, subject: d ? subject.value : "(unreadable message)" };
  confirmOpen.value = true;
}

function confirmDelete(): void {
  if (pendingDelete.value) emit("delete", pendingDelete.value.id);
}

let lastOpenedLink = "";
let lastOpenedAt = 0;

const htmlFrame = ref<HTMLElement | null>(null);
const htmlHost = ref<HTMLElement | null>(null);

/** Keeps mail content inside the frame; see the component doc. */
const FRAME_CONTAINMENT = {
  position: "relative",
  contain: "layout paint",
  isolation: "isolate",
  transform: "translateZ(0)",
} as const;

function openMailLink(url: string): void {
  const resolved = resolveExternalHref(url);
  if (!resolved) return;
  const now = Date.now();
  if (resolved === lastOpenedLink && now - lastOpenedAt < 750) return;
  lastOpenedLink = resolved;
  lastOpenedAt = now;
  void openInBrowser(resolved).catch((err) => {
    toast.error("Couldn't open link", (err as IpcError).message || resolved);
  });
}

function scrollToFragment(fragment: string): void {
  const frame = htmlFrame.value;
  const root = htmlHost.value?.shadowRoot;
  if (!frame || !root) return;
  const target = findFragmentTarget(root, fragment);
  if (!target) return;
  frame.scrollTop += target.getBoundingClientRect().top - frame.getBoundingClientRect().top;
}

function onHtmlLinkClick(ev: Event): void {
  const action = resolveFrameLink(ev.target);
  if (!action) return;
  ev.preventDefault();
  ev.stopPropagation();
  if (action.kind === "open") openMailLink(action.url);
  else if (action.kind === "scroll") scrollToFragment(action.fragment);
}

/**
 * Render sanitized mail HTML into the host's shadow root.
 *
 * A shadow root scopes style *rules* but not inheritance, so the message would
 * otherwise pick up the app shell's typography through the host. The wrapper
 * resets what the old sandboxed iframe used to get for free as a separate
 * document: grayscale font smoothing and `optimizeLegibility` (see
 * `body` in style.css) render mail text noticeably lighter than the sender
 * intended, and the global `user-select: none` would make it uncopyable.
 */
function renderHtmlBody(html: string): void {
  const host = htmlHost.value;
  if (!host) return;
  const { head, body, bodyStyle } = buildMailFrameDocument(html, {
    loadRemoteContent: loadRemote.value,
    localTld: props.tld,
  });
  let root = host.shadowRoot;
  if (!root) {
    root = host.attachShadow({ mode: "open" });
    root.addEventListener("click", onHtmlLinkClick, true);
  }
  root.innerHTML = `<style>
:host { display: block; }
.yerd-mail-body {
  min-height: 100%;
  padding: 8px;
  box-sizing: border-box;
  color: #111;
  font: 14px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  -webkit-font-smoothing: auto;
  text-rendering: auto;
  font-feature-settings: normal;
  font-variant-ligatures: normal;
  -webkit-user-select: text;
  user-select: text;
}
.yerd-mail-body img, .yerd-mail-body table { max-width: 100%; }
.yerd-mail-body a { cursor: pointer; }
</style>${head}<div class="yerd-mail-body">${body}</div>`;
  const wrapper = root.querySelector<HTMLElement>(".yerd-mail-body");
  if (wrapper && bodyStyle) wrapper.setAttribute("style", bodyStyle);
  propagateMailBackground(wrapper);
}

/**
 * Mirror CSS background propagation: a document's `<body>` background paints
 * the whole canvas, so the mail's background must reach the scroll container
 * rather than stopping at the end of its content.
 */
function propagateMailBackground(wrapper: HTMLElement | null): void {
  const frame = htmlFrame.value;
  if (!frame) return;
  frame.style.removeProperty("background-color");
  if (!wrapper) return;
  const background = getComputedStyle(wrapper).backgroundColor;
  if (!background || background === "transparent") return;
  if (/^rgba\(.*,\s*0\)$/.test(background)) return;
  frame.style.backgroundColor = background;
}

function clearHtmlBody(): void {
  htmlFrame.value?.style.removeProperty("background-color");
  const root = htmlHost.value?.shadowRoot;
  if (root) root.innerHTML = "";
}

watch(
  [() => props.detail?.html_body, () => props.loading, htmlHost, loadRemote, () => props.tld],
  ([html, loading]) => {
    if (loading) return;
    if (!html) {
      clearHtmlBody();
      return;
    }
    void nextTick(() => renderHtmlBody(html));
  },
  { immediate: true },
);

function handleTextLinkClick(ev: MouseEvent): void {
  const action = resolveFrameLink(ev.target);
  if (!action) return;
  ev.preventDefault();
  if (action.kind === "open") openMailLink(action.url);
}

async function openAttachment(att: MailAttachment): Promise<void> {
  try {
    const path = await saveMailAttachment(att.filename, att.data);
    await openInEditor(path);
  } catch (e) {
    toast.error("Couldn't open attachment", (e as IpcError).message || att.filename);
  }
}

const TRIGGER =
  "flex h-[26px] items-center rounded-[5px] px-3 text-xs text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-40 data-[state=active]:bg-card data-[state=active]:font-semibold data-[state=active]:text-foreground data-[state=active]:shadow-sm";
const PANEL = "h-full overflow-auto rounded-[10px] border focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
</script>

<template>
  <div class="flex h-full min-h-0 flex-col px-7 pb-6 pt-[22px]" data-testid="mail-reader-body">
    <div v-if="loading" class="flex flex-1 items-center justify-center">
      <Spinner class="size-6" />
    </div>

    <template v-else-if="detail">
      <div class="flex shrink-0 items-start gap-3">
        <div class="min-w-0 flex-1">
          <h2 class="line-clamp-2 text-[19px] font-semibold leading-snug tracking-[-0.2px]" data-testid="reader-subject">
            {{ subject }}
          </h2>
          <p class="mt-1.5 flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-xs text-muted-foreground" data-testid="reader-meta">
            <template v-if="sender.name">
              <span class="font-semibold text-foreground">{{ sender.name }}</span>
              <span class="font-mono">&lt;{{ sender.address }}&gt;</span>
            </template>
            <span v-else class="font-mono">{{ sender.address }}</span>
            <ArrowRight class="size-3 shrink-0" aria-label="to" />
            <span class="font-mono">{{ detail.to.join(", ") }}</span>
            <span aria-hidden="true">·</span>
            <span>{{ readerTime(detail.date_epoch, now) }}</span>
          </p>
        </div>
        <Button variant="outline" size="sm" :disabled="deleting" @click="askDelete">
          <Trash2 />
          Delete
        </Button>
        <Button
          v-if="closable"
          variant="outline"
          size="icon"
          class="size-8"
          aria-label="Close message"
          title="Close"
          @click="emit('close')"
        >
          <X />
        </Button>
      </div>

      <div
        v-if="attachments.length > 0"
        class="mt-4 shrink-0 rounded-[10px] border bg-muted/40 px-4 py-3"
        data-testid="reader-attachments"
      >
        <p class="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Paperclip class="size-3.5" aria-hidden="true" />
          {{ attachmentsLabel(attachments.length) }}
        </p>
        <div class="mt-2 flex flex-wrap gap-2">
          <button
            v-for="(att, i) in attachments"
            :key="`${i}:${att.filename}`"
            type="button"
            class="flex items-center gap-2.5 rounded-lg border bg-card px-3 py-2 text-left transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            :title="`Open ${att.filename} (${humaniseBytes(att.size)})`"
            @click="void openAttachment(att)"
          >
            <FileDown class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
            <span class="flex min-w-0 flex-col leading-tight">
              <span class="max-w-[14rem] truncate text-xs font-semibold">{{ att.filename }}</span>
              <span class="mt-0.5 text-[11px] text-faint">
                {{ mimeLabel(att.content_type) }} · {{ humaniseBytes(att.size) }}
              </span>
            </span>
          </button>
        </div>
      </div>

      <TabsRoot v-model="tab" class="mt-4 flex min-h-0 flex-1 flex-col">
        <div class="flex shrink-0 flex-wrap items-center gap-3">
          <TabsList
            aria-label="Message view"
            class="flex items-center gap-0.5 rounded-[7px] border bg-muted p-0.5"
          >
            <TabsTrigger value="preview" :class="TRIGGER" :disabled="!detail.html_body">Preview</TabsTrigger>
            <TabsTrigger value="text" :class="TRIGGER">Text</TabsTrigger>
            <TabsTrigger value="headers" :class="TRIGGER">Headers</TabsTrigger>
          </TabsList>
          <p
            v-if="tab === 'preview' && remoteLabel"
            class="ml-auto flex items-center gap-2 text-xs text-muted-foreground"
            data-testid="remote-bar"
          >
            <ImageOff class="size-3.5 shrink-0" aria-hidden="true" />
            <span>{{ remoteLabel }}</span>
            <button
              type="button"
              class="font-semibold text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              :aria-pressed="loadRemote"
              :title="loadRemote ? 'Block remote images and stylesheets again' : 'Load remote images and stylesheets for this message'"
              @click="loadRemote = !loadRemote"
            >
              {{ loadRemote ? "Hide" : "Load" }}
            </button>
          </p>
        </div>

        <div class="mt-4 min-h-0 flex-1">
          <TabsContent value="preview" force-mount class="h-full data-[state=inactive]:hidden">
            <div ref="htmlFrame" :class="[PANEL, 'bg-white']" :style="FRAME_CONTAINMENT" data-testid="reader-frame">
              <div ref="htmlHost" data-testid="reader-html" />
            </div>
          </TabsContent>
          <TabsContent value="text" class="h-full">
            <!-- eslint-disable-next-line vue/no-v-html -->
            <pre
              :class="[PANEL, 'whitespace-pre-wrap bg-card p-4 font-mono text-xs leading-5']"
              data-testid="reader-text"
              @click="handleTextLinkClick"
              v-html="linkedTextBody"
            />
          </TabsContent>
          <TabsContent value="headers" class="h-full">
            <div :class="[PANEL, 'bg-card p-4']" data-testid="reader-headers">
              <dl class="grid grid-cols-[110px_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-xs">
                <template v-for="(h, i) in detail.headers" :key="i">
                  <dt class="truncate text-faint" :title="h.name">{{ h.name }}</dt>
                  <dd class="break-all font-mono">{{ h.value }}</dd>
                </template>
              </dl>
              <template v-if="remoteContent.length > 0">
                <p class="mt-4 border-t pt-3 text-[11px] font-semibold uppercase tracking-[0.6px] text-faint">
                  Remote content
                </p>
                <dl class="mt-2 grid grid-cols-[110px_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-xs">
                  <template v-for="item in remoteContent" :key="`${item.kind}:${item.url}`">
                    <dt class="text-faint">{{ remoteKindLabel(item.kind) }}</dt>
                    <dd class="break-all font-mono" :title="item.url">{{ item.url }}</dd>
                  </template>
                </dl>
              </template>
            </div>
          </TabsContent>
        </div>
      </TabsRoot>
    </template>

    <div
      v-else-if="failedId"
      class="flex flex-1 flex-col items-center justify-center gap-3 text-center"
      data-testid="reader-failed"
    >
      <p class="text-[13px] text-muted-foreground">This message couldn't be opened.</p>
      <Button variant="outline" size="sm" :disabled="deleting" @click="askDelete">
        <Trash2 />
        Delete
      </Button>
    </div>

    <p v-else class="flex flex-1 items-center justify-center text-[13px] text-faint">
      Select a message to read it.
    </p>

    <ConfirmDialog
      v-model:open="confirmOpen"
      title="Delete this message?"
      confirm-label="Delete"
      tone="danger"
      :icon="Trash2"
      @confirm="confirmDelete"
    >
      “{{ pendingDelete?.subject }}” will be deleted. This can't be undone.
    </ConfirmDialog>
  </div>
</template>
