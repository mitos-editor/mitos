(() => {
  const select = document.getElementById("doc-version");
  if (!select) return;

  const current = select.value;
  select.addEventListener("change", async () => {
    const version = select.value;
    select.disabled = true;
    const target = `/docs/${version}/`;
    try {
      const response = await fetch("/docs/versions.json");
      if (!response.ok) throw new Error("Version manifest unavailable");
      const versions = await response.json();
      const root = new URL(path_to_root || "./", location.href).pathname;
      const page = location.pathname.slice(root.length) || "index.html";
      // Workers serves clean URLs as well as the original mdBook .html URLs.
      const file = page.endsWith("/") ? `${page}index.html` : page.endsWith(".html") ? page : `${page}.html`;
      location.assign(versions[version]?.includes(file) ? `${target}${file}${location.search}${location.hash}` : target);
    } catch {
      location.assign(target);
    } finally {
      select.value = current;
      select.disabled = false;
    }
  });
})();
