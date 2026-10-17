// Shows the section the URL fragment names, so links, Back, and Forward switch pages.
(() => {
  const sections = [...document.querySelectorAll("main > section")];
  const links = [...document.querySelectorAll("nav a")];
  // Each page's scroll position, so returning to Credits keeps the list where it was.
  const positions = new Map();
  let current = null;
  const show = () => {
    const name = location.hash.slice(1);
    const target = sections.find((section) => section.id === name) || sections[0];
    if (current) {
      positions.set(current, window.scrollY);
    }
    current = target;
    for (const section of sections) {
      section.classList.toggle("current", section === target);
    }
    // A crate's license highlights Credits, the page it was opened from.
    const page = target.dataset.parent || target.id;
    for (const link of links) {
      link.classList.toggle("current", link.dataset.page === page);
    }
    document.title = target.querySelector("h1").textContent;
    window.scrollTo(0, positions.get(target) || 0);
  };
  window.addEventListener("hashchange", show);
  show();
})();
