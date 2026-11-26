/**
 * A one-shot request to open a Services-page dialog from elsewhere (the
 * Overview's "Add service"). Set it, then navigate to `/services`; ServicesView
 * consumes and clears it on mount (or while already mounted).
 */
import { ref } from "vue";

export type ServicesIntent = "add";

export const servicesIntent = ref<ServicesIntent | null>(null);
