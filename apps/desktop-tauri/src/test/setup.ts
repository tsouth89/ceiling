import "@testing-library/jest-dom/vitest";
import "vitest-axe/extend-expect";
import * as axeMatchers from "vitest-axe/matchers";
import { expect } from "vitest";

expect.extend(axeMatchers);
