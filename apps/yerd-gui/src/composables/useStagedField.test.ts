import { describe, expect, it } from "vitest";

import { useStagedField } from "./useStagedField";

const listKey = (v: string[]) => v.join(",");

describe("useStagedField", () => {
  it("follows the saved value while clean", () => {
    const f = useStagedField("always");
    f.sync("trigger");
    expect(f.draft.value).toBe("trigger");
    expect(f.dirty.value).toBe(false);
  });

  it("keeps an edit across a refresh and stays dirty against the new saved value", () => {
    const f = useStagedField(["debug"], listKey);
    f.edit(["debug", "profile"]);
    f.sync(["debug", "trace"]);
    expect(f.draft.value).toEqual(["debug", "profile"]);
    expect(f.dirty.value).toBe(true);
  });

  it("shows the latest saved value when the edit is reverted to where it started", () => {
    const f = useStagedField(["debug"], listKey);
    f.edit(["debug", "profile"]);
    f.sync(["debug", "trace"]);
    f.edit(["debug"]);
    expect(f.draft.value).toEqual(["debug", "trace"]);
    expect(f.dirty.value).toBe(false);
  });

  it("goes clean when a refresh saves exactly the edited value", () => {
    const f = useStagedField("always");
    f.edit("trigger");
    f.sync("trigger");
    expect(f.dirty.value).toBe(false);
    f.sync("always");
    expect(f.draft.value).toBe("always");
  });

  it("drops the edit on reset", () => {
    const f = useStagedField("always");
    f.edit("trigger");
    f.reset("always");
    expect(f.draft.value).toBe("always");
    expect(f.dirty.value).toBe(false);
  });
});
