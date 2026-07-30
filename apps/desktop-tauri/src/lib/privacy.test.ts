import { describe, expect, it } from "vitest";
import { maskEmail, maskEmailsIn, maskIdentity } from "./privacy";
import { accountIdentityLabel } from "./providerRow";

describe("maskEmail", () => {
  it("keeps the domain so an account stays recognisable to its owner", () => {
    expect(maskEmail("bts@cssi.us")).toBe("b••@cssi.us");
  });

  it("does not leak a one-character local part", () => {
    // "a@x.com" masked as "a@x.com" would be no masking at all.
    expect(maskEmail("a@example.com")).toBe("••••@••••");
  });
});

describe("maskEmailsIn", () => {
  it("masks an address embedded in a longer label", () => {
    // Labels are auto-derived as "email (plan)", so the address is a second
    // copy that masking the accountEmail field alone would miss.
    expect(maskEmailsIn("tsouth2@gmail.com (prolite)")).toBe(
      "t••••••@gmail.com (prolite)",
    );
  });

  it("leaves a hand-typed label alone", () => {
    expect(maskEmailsIn("Work")).toBe("Work");
  });
});

describe("maskIdentity", () => {
  it("passes text through untouched when the setting is off", () => {
    expect(maskIdentity("bts@cssi.us", false)).toBe("bts@cssi.us");
  });

  it("returns null for absent text rather than a masked empty string", () => {
    expect(maskIdentity(null, true)).toBeNull();
    expect(maskIdentity("", true)).toBeNull();
  });
});

describe("accountIdentityLabel", () => {
  const withEmail = {
    accountEmail: "bts@cssi.us",
    accountLabel: null,
    planName: "max",
  };

  it("shows the real identity by default", () => {
    expect(accountIdentityLabel(withEmail)).toBe("bts@cssi.us (max)");
  });

  it("masks when Hide Personal Info is on", () => {
    expect(accountIdentityLabel(withEmail, true)).toBe("b••@cssi.us (max)");
  });

  it("masks an email that arrives via the label fallback", () => {
    // The regression: accountEmail was masked but accountLabel, which the app
    // auto-fills with the same address, was printed raw.
    expect(
      accountIdentityLabel(
        {
          accountEmail: null,
          accountLabel: "tsouth2@gmail.com (prolite)",
          planName: null,
        },
        true,
      ),
    ).toBe("t••••••@gmail.com (prolite)");
  });

  it("leaves a nickname readable", () => {
    expect(
      accountIdentityLabel(
        { accountEmail: null, accountLabel: "Work", planName: null },
        true,
      ),
    ).toBe("Work");
  });

  it("still returns null when there is no identity at all", () => {
    expect(
      accountIdentityLabel(
        { accountEmail: null, accountLabel: null, planName: "max" },
        true,
      ),
    ).toBeNull();
  });
});
