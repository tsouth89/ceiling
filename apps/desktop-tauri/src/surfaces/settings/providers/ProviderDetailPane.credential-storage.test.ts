import { describe, expect, it } from "vitest";
import type { LocaleKey } from "../../../i18n/keys";
import { storageLabel } from "./ProviderDetailPane";

const t = (key: LocaleKey) => key;

describe("provider credential storage labels", () => {
  it("does not present another provider's protected file as this provider's credential", () => {
    expect(
      storageLabel(
        {
          fileStatus: "protected:windows-dpapi-user",
          hasProviderCredentials: false,
        },
        t,
      ),
    ).toBe("CredentialStatusNotCreated");
  });

  it("shows protection only when this provider has a credential", () => {
    expect(
      storageLabel(
        {
          fileStatus: "protected:windows-dpapi-user",
          hasProviderCredentials: true,
        },
        t,
      ),
    ).toBe("CredentialProtectedPrefix (windows-dpapi-user)");
  });

  it("does not claim absence when provider presence could not be read", () => {
    expect(
      storageLabel(
        {
          fileStatus: "protected:windows-dpapi-user",
          hasProviderCredentials: null,
        },
        t,
      ),
    ).toBe("CredentialStatusUnreadable");
  });
});
