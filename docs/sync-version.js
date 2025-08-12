#!/usr/bin/env node

import { readFileSync, writeFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

// Read version from Cargo.toml
function getVersionFromCargoToml() {
  const cargoPath = join(__dirname, '../anchor/Cargo.toml');
  const cargoContent = readFileSync(cargoPath, 'utf8');
  const versionMatch = cargoContent.match(/^version = "([^"]+)"/m);
  
  if (!versionMatch) {
    throw new Error('Could not find version in Cargo.toml');
  }
  
  return versionMatch[1];
}

// Update files with the correct version
function updateVersionInFiles(version) {
  const vVersion = `v${version}`;
  
  // Update index.mdx
  const indexPath = join(__dirname, 'docs/pages/index.mdx');
  let indexContent = readFileSync(indexPath, 'utf8');
  
  // Replace download URL version
  indexContent = indexContent.replace(
    /wget https:\/\/github\.com\/sigp\/anchor\/releases\/download\/v[\d.]+\//g,
    `wget https://github.com/sigp/anchor/releases/download/${vVersion}/`
  );
  
  // Replace stats section version
  indexContent = indexContent.replace(
    /<div class="stat-number">v[\d.]+<\/div>/g,
    `<div class="stat-number">${vVersion}</div>`
  );
  
  writeFileSync(indexPath, indexContent);
  
  // Update vocs.config.ts
  const vocsPath = join(__dirname, 'vocs.config.ts');
  let vocsContent = readFileSync(vocsPath, 'utf8');
  
  vocsContent = vocsContent.replace(
    /text: 'v[\d.]+'/g,
    `text: '${vVersion}'`
  );
  
  writeFileSync(vocsPath, vocsContent);
  
  console.log(`✅ Updated version to ${vVersion} in:
  - docs/pages/index.mdx
  - vocs.config.ts`);
}

// Main execution
try {
  const version = getVersionFromCargoToml();
  updateVersionInFiles(version);
} catch (error) {
  console.error('❌ Error syncing version:', error.message);
  process.exit(1);
}