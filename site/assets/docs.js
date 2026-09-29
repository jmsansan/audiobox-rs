import { $, highlight, setupCopy } from './common.js';
setupCopy();
document.querySelectorAll('.language-rust').forEach((code) => highlight(code));
const links = [...$('docs-nav').querySelectorAll('a')];
$('docs-search').addEventListener('input', () => {
  const query = $('docs-search').value.trim().toLowerCase();
  let matches = 0;
  links.forEach((link) => {
    const section = link.hash ? document.querySelector(link.hash) : null;
    const terms = `${link.textContent} ${section?.textContent || ''}`.toLowerCase();
    const visible = !query || terms.includes(query);
    link.hidden = !visible;
    if (visible) matches++;
  });
  $('search-status').textContent = query ? matches ? `${matches} matching sections` : 'No matching sections. Try “gain”, “PCM” or “FLAC”.' : '';
});
