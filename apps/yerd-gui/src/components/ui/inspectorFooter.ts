import type { InjectionKey } from "vue";

/** The selector of the enclosing `InspectorFrame`'s footer bar, which `InspectorFooter` teleports into. */
export const INSPECTOR_FOOTER_KEY: InjectionKey<string> = Symbol("inspector-footer");
