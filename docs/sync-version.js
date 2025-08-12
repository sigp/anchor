#!/usr/bin/env node

import { readFileSync, writeFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

// Fetch GitHub stars count
async function getGitHubStars() {
  try {
    const response = await fetch('https://api.github.com/repos/sigp/anchor');
    if (!response.ok) {
      throw new Error(`GitHub API request failed: ${response.status}`);
    }
    const data = await response.json();
    return data.stargazers_count;
  } catch (error) {
    console.warn('⚠️  Could not fetch GitHub stars, using fallback:', error.message);
    return '45'; // Fallback to current value
  }
}

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

// Update files with the correct version and stars
async function updateVersionAndStarsInFiles(version, stars) {
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
    /<div className="stat-number">v[\d.]+<\/div>/g,
    `<div className="stat-number">${vVersion}</div>`
  );
  
  // Replace stars count
  indexContent = indexContent.replace(
    /<div className="stat-number">\d+<\/div>\s*<div className="stat-label">Stars<\/div>/g,
    `<div className="stat-number">${stars}</div>\n        <div className="stat-label">Stars</div>`
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
  
  console.log(`✅ Updated version to ${vVersion} and stars to ${stars} in:
  - docs/pages/index.mdx
  - vocs.config.ts`);
}

// Main execution
async function main() {
  try {
    const version = getVersionFromCargoToml();
    const stars = await getGitHubStars();
    await updateVersionAndStarsInFiles(version, stars);
  } catch (error) {
    console.error('❌ Error syncing version and stars:', error.message);
    process.exit(1);
  }
}

main();