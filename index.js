const https = require('https');
const url = process.env.URL;
const fs = require('fs');
const helpers = require('./helpers');

https.get(`https://api.hackertarget.com/pagelinks/?q=${url}`, (resp) => {
  let data = '';


  resp.on('data', (chunk) => {
    data += chunk;
  });

  resp.on('end', () => {
    
    let cleanUrls =  extractUrls(data)
    let tasks = helpers.generateTasks(cleanUrls);
    let pythonFile = helpers.generateFile(tasks);
    extractFile(pythonFile);
  });

}).on("error", (err) => {
  console.log("Error: " + err.message);
});


function extractUrls(urls){
    let cleanArray = urls.split("\n");
    let usedLinks = []
    for (let index = 0; index < cleanArray.length; index++) {
      if(cleanArray[index].startsWith(url))
      usedLinks.push(encodeURI(cleanArray[index].split(url)[1]))
    }
    return usedLinks;
    } 
        
function extractFile(python){
  
fs.writeFile("./loc.py", python,'utf8', function(err) {
    if(err) {
        return console.log(err);
    }
    console.log("done !");
}); 
}



