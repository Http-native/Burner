r" ____                                           
/\  _`\                                         
\ \ \L\ \  _ __   __  __    ___      __   _ __  
 \ \  _ <'/\`'__\/\ \/\ \ /' _ `\  /'__`\/\`'__\
  \ \ \L\ \ \ \/ \ \ \_\ \/\ \/\ \/\  __/\ \ \/ 
   \ \____/\ \_\  \ \____/\ \_\ \_\ \____\\ \_\ 
    \/___/  \/_/   \/___/  \/_/\/_/\/____/ \/_/ 
Copyright (c) 2025-present Native people labs.
Engine: 1-dep-rs
                                                
                                                "   

### Use a nicer spinner lib.

---
| Commands availables are. 
---
  |- ✦ burner link -url "http://server" -p 9771
  |- ✦ burner deploy <service-name> -c "<command>" -l "<location>"
  |- ✦ burner run -c "<command>" -l "<location>" [--name <service-name>]
  

burner task runner is a simple task runner that allows you to run commands on the server. You can define tasks in your burner config file and then run them using the burner CLI.

burner run -f <path to task>

burner run -f tasks/build.ts

```ts
export default {
    // Define a task here.
    local: {
        command: "npm run build",
        cwd: "./"
    },
    // Burner allows you to run tasks on the server aswell.
    remote: {
        command: "npm run build",
        cwd: "./"
        files: [], // optional: specify files to be transferred to the server before running the command
        id: "specific id for that server" // the id of the server this remote action will run on.
    }
}
```

burner job api.

burner api:job:create -auth <token>
burner api:job:list
burner api:job:delete
