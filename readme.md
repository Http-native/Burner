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
  name: 'Unstable Azure Server SGP',
  id: 'sgp-1-unstable',
  // exact path is needed
  dirs: ['/etc/daemon/storage', '/var/lib/docker'],
  // optional: specify a backup location (defaults to /backups)
  localBackupDir: '/backups',
  // optional: specify a remote backup location (defaults to /backups on the server)
  remote: {
    1: {
        name: 'backup-us-east',
        host: 'backup-us-east.example.com',
        token: 'your-backup-token',
    },
    2: {
        name: 'backup-eu-west',
        host: 'backup-eu-west.example.com',
        token: 'your-backup-token',
    },
  },
  schedule: {
    // cron expression for scheduling backups (e.g., "0 0 * * *" for daily at midnight)
    cron: "0 0 * * *",
    // or use a simple interval (e.g., "24h" for every 24 hours)
    interval: "24h",
  }
}

export default burner.backup;
```

To restore from a backup do

```bash
burner backup list sgp-1-unstable --target <remote-name> --token <backup-remote-token>
burner backup restore sgp-1-unstable --backup latest/get an id --target <remote-name> --token <backup-remote-token>
```

Here burner will slowly propgate files back into the server's file system. Be aware things like /<user>/ are found, burner will ask for your premission to propgate these. 


## Bruner patches.

Ship delta changes, not the whole app. Burner can detect changes in your application and only deploy the changed files. This is especially useful for large applications where only a few files change between deployments.


```ts
export default {
  ignore: ['node_modules', 'dist', '.env'],
  // ship deltas instead of the whole app every time. This is much faster and more efficient.
  // Note: This only works if there is a build step at the server.
  // If not burner will instantly error out since we ain't js shipping bins
  ship: 'delta', // or 'full' to ship the entire app every time
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
````