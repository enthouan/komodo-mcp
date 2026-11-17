import { defineRouteMiddleware } from "@astrojs/starlight/route-data";

const WEBSITE_NAME = "komodo-mcp";

export const onRequest = defineRouteMiddleware(async (context, next) => {
  await next();

  const route = context.locals.starlightRoute;
  if (route.id === "404") {
    route.head = route.head.flatMap((entry) => {
      if (entry.tag === "link" && entry.attrs?.rel === "canonical") return [];
      if (entry.tag === "meta" && entry.attrs?.property === "og:url") return [];
      if (entry.tag === "meta" && entry.attrs?.name === "robots") {
        return [{ ...entry, attrs: { ...entry.attrs, content: "noindex, nofollow" } }];
      }
      return [entry];
    });
    return;
  }

  if (route.entry.id !== "") return;

  const canonicalHref = route.head.find(
    (entry) => entry.tag === "link" && entry.attrs?.rel === "canonical",
  )?.attrs?.href;

  route.head = route.head.map((entry) =>
    entry.tag === "meta" && entry.attrs?.property === "og:type"
      ? { ...entry, attrs: { ...entry.attrs, content: "website" } }
      : entry,
  );

  if (typeof canonicalHref === "string") {
    const schema = {
      "@context": "https://schema.org",
      "@type": "WebSite",
      name: WEBSITE_NAME,
      url: canonicalHref,
      description: route.entry.data.description,
    };
    route.head.push({
      tag: "script",
      attrs: { type: "application/ld+json" },
      content: JSON.stringify(schema).replaceAll("<", "\\u003c"),
    });
  }
});
