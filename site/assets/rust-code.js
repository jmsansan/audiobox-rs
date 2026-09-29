export function rustCode(c, { format = 0, filename = "input.wav", isFile = false } = {}) {
  const ext = ['wav', 'flac', 'mp3'][format];
  const imports = ['EncodeOptions', 'read_audio_file', 'write_audio_file'];
  if (c.fadeIn || c.fadeOut) imports.push('FadeCurve');
  if (c.filter) imports.push('FilterOptions', 'FilterType');
  if (c.normalize) imports.push('NormalizeOptions');
  const lines = [`use audiobox::{${imports.join(', ')}};`, '', 'fn main() -> audiobox::Result<()> {'];
  if (!isFile) lines.push('    // Use a recording of your own in place of the generated demo.');
  lines.push(`    let audio = read_audio_file(${JSON.stringify(filename)})?;`, '    let result = audio', `        .cut(${c.start.toFixed(4)}, ${c.end.toFixed(4)})?`);
  if (c.mono) lines.push('        .to_mono()');
  if (c.reverse) lines.push('        .reverse()');
  if (c.rate) lines.push(`        .resample(${c.rate.toLocaleString('en-US').replaceAll(',', '_')})?`);
  lines.push(`        .gain_db(${c.gain.toFixed(1)})?`);
  if (c.filter) lines.push(`        .filter(FilterOptions::new(FilterType::${c.filter === 1 ? 'Lowpass' : 'Highpass'}, ${c.frequency.toFixed(1)}))?`);
  lines.push(`        .fade(${c.fadeIn.toFixed(1)}, ${c.fadeOut.toFixed(1)}, FadeCurve::EqualPower)?`);
  // fade is emitted even when both values are zero, so import the curve too.
  if (!imports.includes('FadeCurve')) lines[0] = `use audiobox::{${[...imports, 'FadeCurve'].join(', ')}};`;
  if (c.normalize) lines.push('        .normalize(NormalizeOptions::default())?');
  lines[lines.length - 1] += ';';
  lines.push('', `    write_audio_file("processed.${ext}", &result,`, '        &EncodeOptions::default())?;', '    Ok(())', '}');
  return lines.join('\n');
}
