#!/usr/bin/env node

import { readFileSync, writeFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

// Fetch GitHub repository stats (stars and contributors)
async function getGitHubStats() {
  try {
    // Fetch repository info for stars
    const repoResponse = await fetch('https://api.github.com/repos/sigp/anchor');
    if (!repoResponse.ok) {
      throw new Error(`GitHub repo API request failed: ${repoResponse.status}`);
    }
    const repoData = await repoResponse.json();
    
    // Fetch contributors for count
    const contributorsResponse = await fetch('https://api.github.com/repos/sigp/anchor/contributors');
    if (!contributorsResponse.ok) {
      throw new Error(`GitHub contributors API request failed: ${contributorsResponse.status}`);
    }
    const contributorsData = await contributorsResponse.json();
    
    return {
      stars: repoData.stargazers_count,
      contributors: contributorsData.length
    };
  } catch (error) {
    console.warn('⚠️  Could not fetch GitHub stats, using fallback:', error.message);
    return {
      stars: '46', // Fallback to current value
      contributors: '12' // Fallback to current value
    };
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

// Update files with the correct version, stars, and contributors
async function updateVersionAndStatsInFiles(version, stats) {
  const vVersion = `v${version}`;
  
  // Update index.mdx
  const indexPath = join(__dirname, 'docs/pages/index.mdx');
  let indexContent = readFileSync(indexPath, 'utf8');
  
  // Replace download URL version
  indexContent = indexContent.replace(
    /wget https:\/\/github\.com\/sigp\/anchor\/releases\/download\/v[\d.]+(?:-[\w.]+)?\//g,
    `wget https://github.com/sigp/anchor/releases/download/${vVersion}/`
  );
  
  // Replace stats section version
  indexContent = indexContent.replace(
    /<div className="stat-number">v[\d.]+(?:-[\w.]+)?<\/div>/g,
    `<div className="stat-number">${vVersion}</div>`
  );
  
  // Replace stars count
  indexContent = indexContent.replace(
    /<div className="stat-number">\d+<\/div>\s*<div className="stat-label">Stars<\/div>/g,
    `<div className="stat-number">${stats.stars}</div>\n        <div className="stat-label">Stars</div>`
  );
  
  // Replace contributors count
  indexContent = indexContent.replace(
    /<div className="stat-number">\d+<\/div>\s*<div className="stat-label">Contributors<\/div>/g,
    `<div className="stat-number">${stats.contributors}</div>\n        <div className="stat-label">Contributors</div>`
  );
  
  writeFileSync(indexPath, indexContent);
  
  // Update installation.mdx
  const installationPath = join(__dirname, 'docs/pages/installation.mdx');
  let installationContent = readFileSync(installationPath, 'utf8');
  
  // Replace download URL versions
  installationContent = installationContent.replace(
    /wget https:\/\/github\.com\/sigp\/anchor\/releases\/download\/v[\d.]+(?:-[\w.]+)?\/anchor-v[\d.]+(?:-[\w.]+)?-/g,
    `wget https://github.com/sigp/anchor/releases/download/${vVersion}/anchor-${vVersion}-`
  );
  
  // Replace tar extraction versions
  installationContent = installationContent.replace(
    /tar -xvf anchor-v[\d.]+(?:-[\w.]+)?-/g,
    `tar -xvf anchor-${vVersion}-`
  );
  
  writeFileSync(installationPath, installationContent);
  
  // Update vocs.config.ts
  const vocsPath = join(__dirname, 'vocs.config.ts');
  let vocsContent = readFileSync(vocsPath, 'utf8');
  
  vocsContent = vocsContent.replace(
    /text: 'v[\d.]+(?:-[\w.]+)?'/g,
    `text: '${vVersion}'`
  );
  
  writeFileSync(vocsPath, vocsContent);
  
  console.log(`✅ Updated version to ${vVersion}, stars to ${stats.stars}, and contributors to ${stats.contributors} in:
  - docs/pages/index.mdx
  - docs/pages/installation.mdx
  - vocs.config.ts`);
}

// Main execution
async function main() {
  try {
    const version = getVersionFromCargoToml();
    const stats = await getGitHubStats();
    await updateVersionAndStatsInFiles(version, stats);
  } catch (error) {
    console.error('❌ Error syncing version and GitHub stats:', error.message);
    process.exit(1);
  }
}

main();