# It works on my Production Server

Burner makes deploying applications easier. Ignore dockploy, shipit, capistrano, and all the other tools that are too complex for your needs. Burner is a simple tool that deploys your application to your production server with a single command.

Define a burner deployment.

```ts
export default {
  ignore: ['node_modules', 'dist', '.env'],
  // Define a build step here.
  build: {
    local: {
        command: "npm run build",
        cwd: "./"
    },
    // Burner allows you to
    // Build on the server aswell.
    remote: {
        command: "npm run build",
        cwd: "./"
    }
  },
  installScript: "apk add nodejs npm",
  // The entry point of your application. 
  // This is the command that will be executed
  //  when you run the application on the server.
  entrypoint: "node dist/index.js",
}
```

## Burner Backups

Burner also supports backups. You can define a backup configuration in your burner config file. This will allow you to create backups of your application before deploying new changes.

```ts
const burner.backup = {
  
}

export default burner.backup;
```