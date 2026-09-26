import { computed } from "vue";

import { siteUrl } from "@/lib/siteUrl";
import type { StatusReport } from "@/ipc/types";

export interface SiteBasicsForm {
  name: string;
  location: string;
  php: string;
  secure: boolean;
}

/**
 * The derived values every create wizard shows for its Basics step. The daemon
 * lowercases the site name and creates the project folder from it, so the
 * domain, path and browser URL all use the lowercased name. `nameValid` mirrors
 * `validate_and_lowercase_name` in `yerd-core` and checks the name as typed:
 * lowercasing first would let Unicode such as the Kelvin sign fold into ASCII.
 */
export function useSiteBasics(
  form: SiteBasicsForm,
  props: { tld: string; report: StatusReport | null },
) {
  const siteName = computed(() => form.name.trim().toLowerCase());
  const nameValid = computed(() =>
    /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i.test(form.name.trim()),
  );
  const projectPath = computed(() => (form.location ? `${form.location}/${siteName.value}` : ""));
  const domain = computed(() => `${siteName.value || "name"}.${props.tld}`);
  const openUrl = computed(() =>
    siteUrl({ name: siteName.value || "name", secure: form.secure }, props.report),
  );
  const basicsValid = computed(
    () => nameValid.value && form.location.trim() !== "" && form.php !== "",
  );
  return { siteName, nameValid, projectPath, domain, openUrl, basicsValid };
}
