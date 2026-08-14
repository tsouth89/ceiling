/** Replace `{}` placeholders in a locale template, left to right. */
export function formatLocale(template: string, ...args: string[]): string {
  let result = template;
  for (const arg of args) {
    const index = result.indexOf("{}");
    if (index === -1) break;
    result = result.slice(0, index) + arg + result.slice(index + 2);
  }
  return result;
}
