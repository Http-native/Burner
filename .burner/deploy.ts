// @ts-check

/**
 * Represents a single build target configuration.
 * Used for both local and remote builds.
 *
 * @typedef {Object} BuildTarget
 * @property {string} command - Shell command to execute for building
 * @property {string} cwd - Working directory where the command runs
 */

/**
 * Main Burner configuration object.
 *
 * @typedef {Object} BurnerConfig
 * @property {string[]} ignore - List of files/directories to exclude from bundling/deploy
 * @property {{
 *   local: BuildTarget,
 *   remote: BuildTarget
 * }} build - Build configuration for local and remote environments
 * @property {string} installScript - Command run on the server to install dependencies
 * @property {string} entrypoint - Command used to start the application
 */

/**
 * @type {BurnerConfig}
 */
const burner = {
  // Files/folders that should not be included in the build/deployment
  ignore: ['node_modules', 'dist', '.env'],

  // Define how the project is built
  build: {
    // Build configuration when running locally
    local: {
      command: 'npm run build', // Command to execute
      cwd: './',                // Run from project root
    },

    // Build configuration when running on the remote/server
    remote: {
      command: 'npm run build',
      cwd: './',
    },
  },

  // Script executed on the server before running the app
  // Typically used to install dependencies
  // maybe packages in docker who knows?
  installScript: "",

  // Entry point command for starting the app on the server
  // Example: "node dist/index.js"
  entrypoint: "node dist/index.js",
}

// Export the config so Burner can load it
export default burner