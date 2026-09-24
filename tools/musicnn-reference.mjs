// Developer-only reference generation; never imported by the application.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { isDeepStrictEqual, parseArgs } from 'node:util';

function main() {
  const { values } = parseArgs({
    options: {
      essentia: { type: 'string' },
      output: { type: 'string' },
      check: { type: 'string' },
    },
  });
  if (!values.essentia || Boolean(values.output) === Boolean(values.check)) {
    throw new Error('Usage: node tools/musicnn-reference.mjs --essentia PACKAGE_DIRECTORY (--check FIXTURE | --output NEW_FIXTURE)');
  }
  const root = path.resolve(values.essentia);
  const manifest = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
  assert.equal(manifest.name, 'essentia.js', 'Wrong reference package');
  assert.equal(manifest.version, '0.1.3', 'Reference version changed; review before regenerating');
  const artifacts = [
    ['essentia-wasm.umd.js', '7e0a2b5507199e8162c4ed090d38518de0c7faa070ce35d1593e7631b201014d'],
    ['essentia.js-core.umd.js', 'e3958a89ca0d3e1f95f67de627a383cdbbd8058f24fd53c6749758ff78b5e337'],
  ];
  for (const [file, expected] of artifacts) {
    const actual = createHash('sha256').update(fs.readFileSync(path.join(root, 'dist', file))).digest('hex');
    assert.equal(actual, expected, 'Reference artifact changed: ' + file);
  }
  const require = createRequire(import.meta.url);
  const EssentiaWASM = require(path.join(root, 'dist', artifacts[0][0]));
  const Essentia = require(path.join(root, 'dist', artifacts[1][0]));
  const essentia = new Essentia(EssentiaWASM);
  const tau = 2 * Math.PI;
  const signals = [
    ['silence', () => 0],
    ['first_impulse', i => i === 0 ? 1 : 0],
    ['center_impulse', i => i === 256 ? 1 : 0],
    ['last_impulse', i => i === 511 ? 1 : 0],
    ['dc_offset', () => 0.125],
    ['tone_1000_hz', i => 0.5 * Math.sin(tau * 1000 * i / 16000)],
    ['quiet_tone', i => 0.0001 * Math.sin(tau * 440 * i / 16000)],
    ['near_nyquist', i => 0.5 * Math.sin(tau * 7900 * i / 16000)],
    ['two_tones', i => 0.4 * Math.sin(tau * 220 * i / 16000) + 0.2 * Math.sin(tau * 3200 * i / 16000)],
    ['chirp', i => 0.5 * Math.sin(tau * (150 * i / 16000 + 6000 * (i / 16000) ** 2 / (2 * 0.032)))],
    ['clipped_square', i => i % 31 < 15 ? 1 : -1],
    ['integer_noise', i => (((Math.imul(i + 1, 1103515245) + 12345) >>> 8) & 65535) / 32768 - 1],
  ];
  try {
    assert.equal(essentia.version, '2.1-beta6-dev');
    const cases = signals.map(([name, signal]) => {
      // Store the exact float32 input, avoiding host-language sine differences in Rust tests.
      const input = Float32Array.from({ length: 512 }, (_, i) => signal(i));
      const frame = essentia.arrayToVector(input);
      let result;
      try {
        result = essentia.TensorflowInputMusiCNN(frame);
        const bands = Array.from(essentia.vectorToArray(result.bands));
        assert.equal(bands.length, 96);
        assert(bands.every(Number.isFinite), 'Non-finite reference output: ' + name);
        return { name, input: Array.from(input), bands };
      } finally {
        result?.bands.delete();
        frame.delete();
      }
    });
    const fixture = {
      schema_version: 'musicnn-frame-reference/v1',
      reference: {
        package: 'essentia.js',
        package_version: '0.1.3',
        git_revision: 'f46c91c08bdf263d5f3d575ab8fb0f9b81695acf',
        essentia_version: essentia.version,
        algorithm: 'TensorflowInputMusiCNN',
        artifact_sha256: artifacts[0][1],
        core_artifact_sha256: artifacts[1][1],
        source: 'https://github.com/MTG/essentia.js/tree/f46c91c08bdf263d5f3d575ab8fb0f9b81695acf',
        scope: 'One 512-sample mono frame at 16 kHz; no decoding, framing, patching or model inference',
      },
      frame_size: 512,
      mel_bands: 96,
      max_absolute_error: 0.0001,
      cases,
    };
    if (values.check) {
      assert(isDeepStrictEqual(JSON.parse(fs.readFileSync(values.check, 'utf8')), fixture),
        'Reference fixture differs; inspect differences before replacing it');
      console.log('Verified ' + cases.length + ' synthetic frames and ' + cases.length * 96 + ' reference features.');
    } else {
      const header = JSON.stringify({ ...fixture, cases: [] }, null, 2);
      const text = header.replace('"cases": []',
        '"cases": [\n' + cases.map(item => '    ' + JSON.stringify(item)).join(',\n') + '\n  ]') + '\n';
      fs.writeFileSync(values.output, text, { flag: 'wx' });
      console.log('Created ' + values.output + '; inspect it before replacing the tracked fixture.');
    }
  } finally {
    essentia.shutdown();
    essentia.delete();
  }
}

try {
  main();
} catch (error) {
  // Avoid dumping complete numerical fixtures or an upstream embedded WASM line.
  console.error(error instanceof Error ? error.message : "Reference generation failed");
  process.exitCode = 1;
}
