const https = require('https');
const url = process.env.URL;
const fs = require('fs');

https.get(`https://api.hackertarget.com/pagelinks/?q=${url}`, (resp) => {
  let data = '';


  resp.on('data', (chunk) => {
    data += chunk;
  });

  resp.on('end', () => {
    
    let cleanUrls =  extractUrls(data)
    let tasks = generateTasks(cleanUrls);
    let pythonFile = generateFile(tasks);
    extractFile(pythonFile);
  });

}).on("error", (err) => {
  console.log("Error: " + err.message);
});


function extractUrls(urls){
    let cleanArray = urls.split("\n");
    let usedLinks = []
    for (let index = 0; index < cleanArray.length; index++) {
      if(cleanArray[index].startsWith(url) && ! isArabic(cleanArray[index].split(url)[1]))
      usedLinks.push(cleanArray[index].split(url)[1])
    }
    return usedLinks;
    } 
    
function generateTasks(links){
  let result = '';
  for (let index = 1; index <= links.length; index++) {
    result += `@task(${index})
    def f${index}(self):
     self.client.get("${links[index]}")
         
    `
  }
  return result;
};    

function generateFile(tasks){
  return `
  from locust import HttpLocust, TaskSet, task


class UserBehavior(TaskSet):

    def fon_start(self):
        self.login()

    def flogin(self):
        self.client.get("/")

    
    ${tasks}

class WebsiteUser(HttpLocust):
    task_set = UserBehavior
    min_wait = 5000
    max_wait = 9000

  `
}

function extractFile(python){
  
fs.writeFile("./locustfile.py", python,'utf8', function(err) {
    if(err) {
        return console.log(err);
    }
    console.log("done !");
}); 
}

function isArabic(text) {
  var pattern = /[\u0600-\u06FF\u0750-\u077F]/;
  result = pattern.test(text);
  return result;
}

